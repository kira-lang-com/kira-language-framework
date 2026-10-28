//! Call lowering: method-shaped `.append`, the builtin families, and direct
//! calls to declared functions with their generic inference and result ABI.

use super::expr::Receiver;
use super::{Builtin, Lowerer};
use crate::Signature;
use crate::methods;
use crate::ty::FeType;
use crate::type_error;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{ConvertKind, Type};
use klf_kira_syntax::ast::{Arg, Expr, ExprKind, Ident};
use klf_source::Span;
use std::collections::HashMap;

impl Lowerer<'_> {
    /// A call; `expected` is the type its value is wanted as, which pins
    /// the type arguments of a generic callee its arguments cannot.
    pub(super) fn lower_call(
        &mut self,
        callee: &Expr,
        args: &[Arg],
        span: Span,
        expected: Option<&FeType>,
    ) -> Option<(Name, FeType)> {
        if let Some(receiver) = super::expr::method_receiver(callee, "flag") {
            return self
                .lower_session_flag(receiver, args, span)
                .map(|(value, ty)| self.fresh(value, ty));
        }
        if let Some(receiver) = super::expr::method_receiver(callee, "file_mode") {
            return self
                .lower_session_file_mode(receiver, args, span)
                .map(|(value, ty)| self.fresh(value, ty));
        }
        if let Some(receiver) = super::expr::method_receiver(callee, "file_memory") {
            return self
                .lower_session_file_memory(receiver, args, span)
                .map(|(value, ty)| self.fresh(value, ty));
        }
        if let Some(receiver) = super::expr::method_receiver(callee, "file_convention") {
            return self
                .lower_session_file_convention(receiver, args, span)
                .map(|(value, ty)| self.fresh(value, ty));
        }
        if let Some(receiver) = super::expr::append_receiver(callee) {
            return self.lower_append(receiver, args, span);
        }
        if let Some((receiver, method)) = super::expr::method_call(callee) {
            return self.lower_method_call(receiver, &method, args, span, expected);
        }
        let ExprKind::Path(path) = &callee.kind else {
            self.session.report(
                Diagnostic::error(
                    "KLFK-E0203",
                    "only direct calls to named functions are supported",
                )
                .with_label(span, "not callable"),
            );
            return None;
        };
        let name = path.segments[0].name.clone();
        if path.segments.len() == 1
            && let Some(local) = self.lookup(&name)
            && matches!(local.ty, FeType::Function { .. })
        {
            let closure = self.lower_path(path, span)?;
            return self.lower_closure_call(closure, args, span);
        }
        if path.segments.len() == 1 {
            if let Some(receiver) = self.lookup(&Name::new("self")) {
                let method = methods::qualified(&receiver.ty, name.as_str());
                if self.signatures.contains_key(&method) && !self.signatures.contains_key(&name) {
                    return self.lower_direct_call(&name, args, span, expected);
                }
            }
            let text = self.try_text_builtin(name.as_str(), args, span);
            if !matches!(text, Builtin::Skipped) {
                return text.into_value().map(|(value, ty)| self.fresh(value, ty));
            }
            let file = self.try_file_builtin(name.as_str(), args, span);
            if !matches!(file, Builtin::Skipped) {
                return file.into_value().map(|(value, ty)| self.fresh(value, ty));
            }
            let editor = self.try_editor_builtin(name.as_str(), args, span);
            if !matches!(editor, Builtin::Skipped) {
                return editor.into_value().map(|(value, ty)| self.fresh(value, ty));
            }
            let cli = self.try_cli_builtin(name.as_str(), args, span);
            if !matches!(cli, Builtin::Skipped) {
                return cli.into_value().map(|(value, ty)| self.fresh(value, ty));
            }
            let process = self.try_process_builtin(name.as_str(), args, span);
            if !matches!(process, Builtin::Skipped) {
                return process
                    .into_value()
                    .map(|(value, ty)| self.fresh(value, ty));
            }
            if crate::testing::is_expect(&name) {
                return self.lower_expect(args, span);
            }
            if name.as_str() == "abort" {
                return self.lower_abort(args, span);
            }
            if name.as_str() == "floatToBits" {
                return self.lower_float_to_bits(args, span);
            }
            if name.as_str() == "bitsToFloat" {
                return self.lower_bits_to_float(args, span);
            }
            if name.as_str() == "artifactHandle" {
                return self.lower_artifact_handle(args, span);
            }
        }
        self.lower_direct_call(&name, args, span, expected)
    }

    fn lower_abort(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        if !args.is_empty() {
            self.session
                .report(type_error(span, "`abort` takes no arguments"));
            return None;
        }
        let value = self.fb.const_int(Type::Integer(64), 0);
        self.trap_reported(
            span,
            &super::trap_report::TrapText {
                code: "KLF-RT-0007",
                headline: "the definition called `abort()`",
                label: "stopped here",
                note: "`abort()` ends the definition on purpose, at a state its author ruled out",
                help: "the frames below show how the definition got here; fix the caller that \
                       reached this state",
            },
        );
        self.reachable = false;
        Some((value, FeType::Void))
    }

    fn lower_optional_unwrap(
        &mut self,
        receiver: (Name, FeType),
        args: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        if !args.is_empty() {
            self.session
                .report(type_error(span, "`.unwrap` takes no arguments"));
            return None;
        }
        let FeType::Enum(owner) = &receiver.1 else {
            self.session
                .report(type_error(span, "`.unwrap` requires an Optional"));
            return None;
        };
        let info = self.enums.get(owner)?;
        let (variant, some) = info.variant("Some")?;
        if info.variant("None").is_none() || info.variants.len() != 2 || some.payload.len() != 1 {
            self.session
                .report(type_error(span, "`.unwrap` requires an Optional"));
            return None;
        }
        let ty = some.payload[0].clone();
        let pass = self.fb.new_block(&[]);
        let fail = self.fb.new_block(&[]);
        self.fb.switch_enum(
            receiver.0.clone(),
            vec![(
                variant,
                klf_kir::BlockTarget {
                    block: pass.clone(),
                    arguments: Vec::new(),
                },
            )],
            Some(klf_kir::BlockTarget {
                block: fail.clone(),
                arguments: Vec::new(),
            }),
        );
        self.fb.switch_to(&fail);
        self.trap_reported(
            span,
            &super::trap_report::TrapText {
                code: "KLF-RT-0007",
                headline: "unwrapped an Optional that holds no value",
                label: "this is `.None`",
                note: "`.unwrap()` reads the value of `.Some` and stops the program on `.None`",
                help: "`match` the Optional and handle `.None`, or give a fallback with `.or(...)`",
            },
        );
        self.fb.switch_to(&pass);
        let address = self.fb.enum_payload_addr(receiver.0, variant);
        let value = if super::lists::is_aggregate(&ty) {
            address
        } else {
            self.fb.load(address)
        };
        Some((value, ty))
    }

    fn lower_float_to_bits(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        if args.len() != 1 {
            self.session
                .report(type_error(span, "`floatToBits` takes one Float"));
            return None;
        }
        let (value, ty) = self.lower_expr(&args[0].value)?;
        if ty != (FeType::Float { bits: 64 }) {
            self.session.report(type_error(
                args[0].value.span,
                "`floatToBits` expects a Float",
            ));
            return None;
        }
        Some((
            self.fb
                .convert(ConvertKind::Bitcast, value, Type::Integer(64)),
            FeType::Int {
                bits: 64,
                signed: true,
            },
        ))
    }

    fn lower_bits_to_float(&mut self, args: &[Arg], span: Span) -> Option<(Name, FeType)> {
        if args.len() != 1 {
            self.session
                .report(type_error(span, "`bitsToFloat` takes one Int"));
            return None;
        }
        let (value, ty) = self.lower_expr(&args[0].value)?;
        if ty
            != (FeType::Int {
                bits: 64,
                signed: true,
            })
        {
            self.session.report(type_error(
                args[0].value.span,
                "`bitsToFloat` expects an Int",
            ));
            return None;
        }
        Some((
            self.fb
                .convert(ConvertKind::Bitcast, value, Type::Float(64)),
            FeType::Float { bits: 64 },
        ))
    }

    /// `receiver.append(value)` — spelled as a field access or a dotted path.
    fn lower_append(
        &mut self,
        receiver: Receiver<'_>,
        args: &[Arg],
        span: Span,
    ) -> Option<(Name, FeType)> {
        if args.len() != 1 {
            self.session
                .report(type_error(span, "`.append` takes one argument"));
            return None;
        }
        let (header, base_ty) = match receiver {
            Receiver::Expr(base) => self.lower_expr(base)?,
            Receiver::Path(path) => self.path_place(&path, span)?,
        };
        let FeType::List(element) = base_ty else {
            let name = methods::qualified(&base_ty, "append");
            return self.lower_resolved_call(&name, Some((header, base_ty)), args, span, None);
        };
        let element = *element;
        self.lower_list_append(&header, &element, &args[0].value)
    }

    /// `value.method(...)` on an instance, or `Owner.method(...)` when the
    /// receiver names a type rather than a value.
    fn lower_method_call(
        &mut self,
        receiver: Receiver<'_>,
        method: &Ident,
        args: &[Arg],
        span: Span,
        expected: Option<&FeType>,
    ) -> Option<(Name, FeType)> {
        if let Some(owner) = self.static_owner(&receiver) {
            let name = methods::qualified(&owner, method.name.as_str());
            return self.lower_resolved_call(&name, None, args, span, expected);
        }
        let receiver = match receiver {
            Receiver::Expr(base) => self.lower_expr(base)?,
            Receiver::Path(path) => self.lower_path(&path, span)?,
        };
        if method.name.as_str() == "unwrap" {
            return self.lower_optional_unwrap(receiver, args, span);
        }
        let name = methods::qualified(&receiver.1, method.name.as_str());
        self.lower_resolved_call(&name, Some(receiver), args, span, expected)
    }

    /// The type a receiver path names, when it is one segment that is not a
    /// value in scope.
    fn static_owner(&mut self, receiver: &Receiver<'_>) -> Option<FeType> {
        let Receiver::Path(path) = receiver else {
            return None;
        };
        let [segment] = path.segments.as_slice() else {
            return None;
        };
        if self.lookup(&segment.name).is_some() {
            return None;
        }
        if self.structs.contains_key(&segment.name) {
            return Some(FeType::Struct(segment.name.clone()));
        }
        if self.enums.contains_key(&segment.name) {
            return Some(FeType::Enum(segment.name.clone()));
        }
        FeType::from_name(segment.name.as_str())
    }

    /// A call by bare name: a free function, or inside a method, a sibling
    /// method called on the same receiver.
    fn lower_direct_call(
        &mut self,
        name: &Name,
        args: &[Arg],
        span: Span,
        expected: Option<&FeType>,
    ) -> Option<(Name, FeType)> {
        if !self.signatures.contains_key(name)
            && let Some(receiver) = self.lookup(&Name::new("self"))
        {
            let owner = receiver.ty.clone();
            let method = methods::qualified(&owner, name.as_str());
            if self.signatures.contains_key(&method) {
                let path = klf_kira_syntax::ast::Path {
                    segments: vec![Ident {
                        name: Name::new("self"),
                        span,
                    }],
                    span,
                };
                let receiver = self.lower_path(&path, span)?;
                return self.lower_resolved_call(&method, Some(receiver), args, span, expected);
            }
        }
        self.lower_resolved_call(name, None, args, span, expected)
    }

    /// A call to a declared function or method. `receiver` is the lowered
    /// `self` of an instance call.
    fn lower_resolved_call(
        &mut self,
        name: &Name,
        receiver: Option<(Name, FeType)>,
        args: &[Arg],
        span: Span,
        expected: Option<&FeType>,
    ) -> Option<(Name, FeType)> {
        let Some(signature) = self.signatures.get(name).cloned() else {
            let kind = if receiver.is_some() {
                "method"
            } else {
                "function"
            };
            self.session.report(
                Diagnostic::error("KLFK-E0200", format!("unknown {kind} `{name}`"))
                    .with_label(span, "not found"),
            );
            return None;
        };
        if signature.receiver != receiver.is_some() {
            let message = if signature.receiver {
                format!("`{name}` takes `self`; call it on a value")
            } else {
                format!("`{name}` has no `self`; call it on its type")
            };
            self.session
                .report(Diagnostic::error("KLFK-E0203", message).with_label(span, "not callable"));
            return None;
        }
        let count = signature.arguments().len();
        if args.len() != count {
            self.session.report(
                Diagnostic::error(
                    "KLFK-E0202",
                    format!("`{name}` expects {count} arguments, got {}", args.len()),
                )
                .with_label(span, "wrong argument count"),
            );
            return None;
        }
        let mut substitutions = HashMap::new();
        if let Some(expected) = expected
            && !signature.generics.is_empty()
        {
            let _ = signature.result.infer_from(expected, &mut substitutions);
        }
        let mut argument_values: Vec<Name> = receiver.map(|(value, _)| value).into_iter().collect();
        argument_values.extend(self.lower_arguments(args, &signature, &mut substitutions)?);
        self.check_inference(name, &signature, &substitutions, span)?;
        let result = signature.result.substitute(&substitutions);
        if let Some(service) = crate::builtins::service(self.session, name.as_str()).cloned() {
            let argument_values = self.text_arguments(&signature, argument_values);
            return Some(self.lower_service_call(&service, argument_values, result));
        }
        if signature.generics.is_empty() {
            return Some(self.emit_call(name, &signature, &substitutions, argument_values, result));
        }
        let instance = super::Instance::of(name, &signature.generics, &substitutions);
        let concrete = Signature {
            generics: Vec::new(),
            bounds: HashMap::new(),
            params: signature
                .params
                .iter()
                .map(|param| param.substitute(&substitutions))
                .collect(),
            result: result.clone(),
            ..signature
        };
        let callee = instance.name.clone();
        self.counts.borrow_mut().request_instance(instance);
        Some(self.emit_call(&callee, &concrete, &HashMap::new(), argument_values, result))
    }

    fn lower_arguments(
        &mut self,
        args: &[Arg],
        signature: &Signature,
        substitutions: &mut HashMap<Name, FeType>,
    ) -> Option<Vec<Name>> {
        let mut values = Vec::with_capacity(args.len());
        let moves = signature.argument_moves();
        for (index, (arg, expected)) in args.iter().zip(signature.arguments()).enumerate() {
            let wanted = expected.substitute(substitutions);
            let (value, ty) = self.lower_expr_expected(&arg.value, Some(&wanted))?;
            // A `move` parameter takes storage the callee releases; any other
            // may see the list an argument sits in grow under it.
            let value = if moves.get(index).copied().unwrap_or(false) && self.counted(expected) {
                self.take_owned(value, &ty)
            } else {
                self.detach(&arg.value, value, &ty)
            };
            if !expected.infer_from(&ty, substitutions) {
                self.session.report(type_error(
                    arg.value.span,
                    format!(
                        "argument has type {}, expected {}",
                        ty.display(),
                        expected.display()
                    ),
                ));
                return None;
            }
            let expected = expected.substitute(substitutions);
            if !self.check_argument_type(&expected, &ty, arg.value.span) {
                return None;
            }
            values.push(value);
        }
        Some(values)
    }

    /// Every generic parameter must be pinned by an argument, and the bounds
    /// the signature declares must hold for what it was pinned to.
    fn check_inference(
        &mut self,
        name: &Name,
        signature: &Signature,
        substitutions: &HashMap<Name, FeType>,
        span: Span,
    ) -> Option<()> {
        for generic in &signature.generics {
            if !substitutions.contains_key(generic) {
                self.session.report(
                    Diagnostic::error(
                        "KLFK-E0202",
                        format!("cannot infer generic argument `{generic}` for `{name}`"),
                    )
                    .with_label(span, "provide an argument whose type determines it"),
                );
                return None;
            }
        }
        self.check_generic_bounds(signature, substitutions, span)
            .then_some(())
    }

    fn emit_call(
        &mut self,
        name: &Name,
        signature: &Signature,
        substitutions: &HashMap<Name, FeType>,
        arguments: Vec<Name>,
        result: FeType,
    ) -> (Name, FeType) {
        let generic_args: Vec<Type> = signature
            .generics
            .iter()
            .filter_map(|generic| substitutions.get(generic).map(FeType::value_kir))
            .collect();
        let generic = !signature.generics.is_empty();
        if let Some(nominal) = sret_nominal(signature, &result) {
            let dest = self.fb.stack_alloc(Type::Nominal(nominal, Vec::new()));
            let mut call_args = Vec::with_capacity(arguments.len() + 1);
            call_args.push(dest.clone());
            call_args.extend(arguments);
            if generic {
                self.fb
                    .call_generic_void(name.clone(), generic_args, call_args);
            } else {
                self.fb.call_void(name.clone(), call_args);
            }
            return self.fresh(dest, result);
        }
        if result == FeType::Void {
            if generic {
                self.fb
                    .call_generic_void(name.clone(), generic_args, arguments);
            } else {
                self.fb.call_void(name.clone(), arguments);
            }
            return (self.fb.const_int(Type::Integer(64), 0), result);
        }
        let value = if generic {
            self.fb.call_generic(name.clone(), generic_args, arguments)
        } else {
            self.fb.call(name.clone(), arguments)
        };
        (value, result)
    }
}

/// The nominal type a call returns through the sret out-pointer, if any.
///
/// Only a concrete aggregate declared return uses that ABI. A generic `-> T`
/// that infers to a struct returns its address by value, exactly as the
/// monomorphized callee was lowered.
fn sret_nominal(signature: &Signature, result: &FeType) -> Option<Name> {
    match &signature.result {
        FeType::Struct(_) | FeType::Enum(_) | FeType::List(_) | FeType::Function { .. } => {
            match result {
                FeType::Struct(name) | FeType::Enum(name) => Some(name.clone()),
                FeType::List(_) => Some(Name::new("List")),
                FeType::Function { .. } => Some(Name::new(crate::ty::CLOSURE_RECORD)),
                _ => None,
            }
        }
        _ => None,
    }
}
