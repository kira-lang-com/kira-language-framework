//! The `ridlcc` entry of every function a library exports. An entry takes
//! its parameters as `ridlcc` passes them and calls the frontend's function
//! in its C view. Owned and `borrow mut` values are adopted in place: a
//! buffer from another allocator is copied into one of the frontend's and
//! released through its `ops.free`. Borrowed values are copied, and the copy
//! is destroyed after the call. Alongside the entries the IR carries a
//! descriptor for every exported type (section 23.1), the module metadata
//! (section 24.3) and a constructor registering it with the runtime.

use super::{
    Body, CView, Ir, Mode, leaf_type, parameters, path, quote, round_up, scalar_type, split_result,
    symbol,
};
use crate::cc::{Allocator, Passing, Registers, classify_result};
use crate::emit::type_key;
use crate::module::{Convention, ExtendBody, FunctionDecl, ItemKind, Module};
use crate::types::{Scalar, Type};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The LLVM IR of the entries, descriptors and registration of `module`,
/// built for `target`. `callees` maps an export's path to the object symbol
/// of the frontend function behind it, `destroys` a type's whitespace-free
/// spelling to its destroy function, and `defined` names the object symbol
/// of the frontend's own helper functions.
///
/// # Errors
/// Returns a message when a type has no layout or an export cannot be
/// adapted.
pub fn entries(
    module: &Module,
    callees: &BTreeMap<String, String>,
    destroys: &BTreeMap<String, String>,
    target: &str,
    defined: &dyn Fn(&str) -> String,
) -> Result<String, String> {
    let registry = module.registry();
    let mut ir = Ir::new(&registry, target, &module.name, destroys, defined, true);
    for item in &module.items {
        match &item.kind {
            // A generic declaration has no layout of its own; each instance
            // has its descriptor where it is used (section 23.5).
            ItemKind::Struct(s) if s.generics.is_empty() => {
                ir.descriptor(&Type::nominal(s.name.clone()))?;
            }
            ItemKind::Enum(e) if e.generics.is_empty() => {
                ir.descriptor(&Type::nominal(e.name.clone()))?;
            }
            _ => {}
        }
    }
    for item in &module.items {
        match &item.kind {
            ItemKind::Function(function) => ir
                .entry(callees, target, None, function)
                .map_err(|why| format!("entry `{}`: {why}", function.name))?,
            ItemKind::Extend(extend) => {
                if let ExtendBody::Methods(methods) = &extend.body {
                    for method in methods {
                        ir.entry(callees, target, Some(&extend.target), method)
                            .map_err(|why| {
                                format!(
                                    "entry `{}.{}`: {why}",
                                    crate::emit::type_text(&extend.target),
                                    method.name
                                )
                            })?;
                    }
                }
            }
            _ => {}
        }
    }
    ir.metadata(module, target);
    Ok(ir.finish())
}

/// How an entry receives `self`.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Receiving {
    /// As its convention says.
    Entry,
    /// A witness entry: the address of the value in its box, handed over as
    /// it is.
    Witness,
    /// A trait's default: the address of the value in its box, handed over
    /// as the trait object the box makes with the witness table.
    Default,
}

/// How one parameter reaches the frontend's function.
struct Lowered {
    /// The value handed to the frontend's function, typed.
    argument: String,
    /// A borrowed copy to destroy after the call.
    destroy: Option<(String, Type)>,
    /// A `borrow mut [T]` copy to write back: copy, caller's data, length.
    write_back: Option<(String, String, String, u64)>,
    /// A `borrow mut` value the caller takes back, made the frontend's alone
    /// after the call.
    unique_after: Option<(String, Type)>,
}

impl Ir<'_> {
    /// The `ridlcc` entry of one exported function or method.
    fn entry(
        &mut self,
        callees: &BTreeMap<String, String>,
        target: &str,
        owner: Option<&Type>,
        function: &FunctionDecl,
    ) -> Result<(), String> {
        let path = path(owner, function);
        let Some(callee) = callees.get(&path) else {
            return Ok(());
        };
        let symbol = symbol(self.module_name, owner, function);
        self.define_entry(target, callee, &symbol, owner, function, Receiving::Entry)
    }

    /// Defines the `ridlcc` function `symbol` calling the frontend's `callee`
    /// in its C view. A witness entry or a trait's default (section 22.6)
    /// takes its receiver as the address of the value in its box, and the
    /// `Self` descriptor and witness table after the declared parameters.
    pub(super) fn define_entry(
        &mut self,
        target: &str,
        callee: &str,
        symbol: &str,
        owner: Option<&Type>,
        function: &FunctionDecl,
        receiving: Receiving,
    ) -> Result<(), String> {
        let witness = receiving != Receiving::Entry;
        let params = parameters(owner, function);
        let (ok, failure) = split_result(function.result.as_ref());
        let view = CView::of(&params, function.result.as_ref(), self.word);
        let has_sret = view.result_address;
        let mut allocator = Allocator::new(Registers::for_target(target));
        let mut body = Body::new();
        let mut signature: Vec<String> = Vec::new();
        let result_leaves = match &ok {
            Some(ty) => Some(self.leaves(ty)?),
            None => None,
        };
        let result_passing = result_leaves.as_deref().map(classify_result);
        let result_type = match (&ok, &result_passing) {
            (Some(Type::Scalar(scalar)), _) => scalar_type(*scalar, self.word),
            (Some(_), Some(Passing::Direct(leaves))) if leaves.len() == 1 => leaf_type(&leaves[0]),
            (Some(_), Some(Passing::Direct(leaves))) => format!(
                "{{ {} }}",
                leaves.iter().map(leaf_type).collect::<Vec<_>>().join(", ")
            ),
            _ => "void".to_owned(),
        };
        if matches!(result_passing, Some(Passing::Indirect)) {
            let (size, _) = self.size_align(ok.as_ref().expect("an indirect result has a type"))?;
            signature.push(format!("ptr noalias sret([{size} x i8]) %result"));
            allocator.take_address();
        }
        let mut lowered = Vec::new();
        for (index, (convention, ty)) in params.iter().enumerate() {
            let kir_param = &view.parameters[index + usize::from(has_sret)];
            if witness && index == 0 {
                allocator.take_address();
                signature.push("ptr %p0".to_owned());
                let argument = if receiving == Receiving::Default {
                    // The trait object the receiver's box makes with the
                    // table, which is what the default takes.
                    let boxed = self.box_of(&mut body, "%p0", "%td");
                    let object = body.alloca(2 * self.word, self.word);
                    body.store("ptr", &boxed, &object);
                    let at = body.at(&object, self.word);
                    body.store("ptr", "%wt", &at);
                    object
                } else {
                    "%p0".to_owned()
                };
                lowered.push(Lowered {
                    argument: format!("ptr {argument}"),
                    destroy: None,
                    write_back: None,
                    unique_after: None,
                });
                continue;
            }
            lowered.push(self.parameter(
                &mut body,
                &mut signature,
                &mut allocator,
                index,
                *convention,
                ty,
                kir_param,
            )?);
        }
        if witness {
            signature.push("ptr %td".to_owned());
            signature.push("ptr %wt".to_owned());
        }
        if failure.is_some() {
            signature.push("ptr swifterror %error".to_owned());
        }
        let mut arguments = Vec::new();
        let returned_storage = if let Some(err) = &failure {
            let whole = Type::Result(
                Box::new(ok.clone().unwrap_or(Type::Tuple(Vec::new()))),
                Box::new(err.clone()),
            );
            let (size, align) = self.size_align(&whole)?;
            let storage = body.alloca(size, align);
            arguments.push(format!("ptr {storage}"));
            Some((storage, whole))
        } else if has_sret {
            let storage = if let Some(Passing::Indirect) = &result_passing {
                "%result".to_owned()
            } else {
                let (size, align) =
                    self.size_align(ok.as_ref().ok_or("a result address without a result")?)?;
                body.alloca(size, align)
            };
            arguments.push(format!("ptr {storage}"));
            Some((storage, ok.clone().unwrap_or(Type::Tuple(Vec::new()))))
        } else {
            None
        };
        arguments.extend(lowered.iter().map(|lowered| lowered.argument.clone()));
        let kir_result = view.result.clone();
        let declared = view.parameters.clone();
        self.declare(
            callee,
            &format!(
                "declare {kir_result} {}({})",
                quote(callee),
                declared.join(", ")
            ),
        );
        let call = format!(
            "call {kir_result} {}({})",
            quote(callee),
            arguments.join(", ")
        );
        let returned = if kir_result == "void" {
            body.line(&call);
            None
        } else {
            let value = body.value();
            body.line(&format!("{value} = {call}"));
            Some(value)
        };
        for lowered in &lowered {
            self.after_call(&mut body, lowered)?;
        }
        // What the function returns is handed over, so it is made the
        // frontend's alone (section 21.4).
        let returned = match (&returned_storage, returned) {
            (Some((storage, whole)), returned) => {
                if self.adopts(whole) {
                    let unique = self.adopter(whole, Mode::Unique)?;
                    body.line(&format!("call void {}(ptr {storage})", quote(&unique)));
                }
                returned
            }
            (None, Some(value)) => match ok.as_ref().filter(|ok| self.adopts(ok)) {
                Some(ok) => {
                    let (size, align) = self.size_align(ok)?;
                    let spilled = body.alloca(size, align);
                    body.store(&kir_result, &value, &spilled);
                    let unique = self.adopter(ok, Mode::Unique)?;
                    body.line(&format!("call void {}(ptr {spilled})", quote(&unique)));
                    Some(body.load(&kir_result, &spilled))
                }
                None => Some(value),
            },
            (None, None) => None,
        };
        if receiving == Receiving::Witness
            && params.first().map(|(convention, _)| *convention) == Some(Convention::Owned)
        {
            // A method taking `self` through a table takes over the whole
            // box; the frontend's method took the value, and the storage
            // goes back through the box's own `free` (section 21.6).
            let boxed = self.box_of(&mut body, "%p0", "%td");
            let free_at = body.at(&boxed, self.word);
            let free = body.load("ptr", &free_at);
            body.line(&format!("call void {free}(ptr {boxed})"));
        }
        match (&failure, &returned_storage) {
            (Some(err), Some((storage, whole))) => {
                self.unpack_failure(
                    &mut body,
                    storage,
                    whole,
                    err,
                    ok.as_ref(),
                    result_passing.as_ref(),
                    &result_type,
                )?;
            }
            _ => match (&result_passing, &returned_storage, returned) {
                (None | Some(Passing::Indirect), _, _) => body.line("ret void"),
                (Some(Passing::Direct(leaves)), Some((storage, _)), _) => {
                    let value = self.load_leaves(&mut body, storage, leaves, &result_type);
                    body.line(&format!("ret {result_type} {value}"));
                }
                (Some(Passing::Direct(_)), None, Some(value)) => {
                    body.line(&format!("ret {result_type} {value}"));
                }
                (Some(Passing::Direct(_)), None, None) => {
                    return Err(format!("`{callee}` returns nothing for a RIDL result"));
                }
            },
        }
        let linkage = if receiving == Receiving::Witness {
            "private "
        } else {
            ""
        };
        let _ = write!(
            self.functions,
            "define {linkage}swiftcc {result_type} {}({}) {{\nentry:\n{}}}\n\n",
            quote(symbol),
            signature.join(", "),
            body.text
        );
        Ok(())
    }

    /// Lowers one parameter: its `ridlcc` form in `signature`, and what the
    /// frontend's function is handed.
    #[allow(clippy::too_many_arguments)]
    fn parameter(
        &mut self,
        body: &mut Body,
        signature: &mut Vec<String>,
        allocator: &mut Allocator,
        index: usize,
        convention: Convention,
        ty: &Type,
        kir_param: &str,
    ) -> Result<Lowered, String> {
        let name = format!("%p{index}");
        let kir_ty = kir_param.to_owned();
        let by_address = kir_ty == "ptr" && !matches!(ty, Type::Scalar(Scalar::RawPtr));
        let mut lowered = Lowered {
            argument: String::new(),
            destroy: None,
            write_back: None,
            unique_after: None,
        };
        let word = self.word;
        let iw = self.iw();
        if let Type::Scalar(scalar) = ty {
            let leaves = self.leaves(ty)?;
            if convention == Convention::BorrowMut {
                allocator.take_address();
                signature.push(format!("ptr {name}"));
                lowered.argument = format!("ptr {name}");
                return Ok(lowered);
            }
            allocator.classify(&leaves, self.size_align(ty)?.0, word, true);
            let llvm = scalar_type(*scalar, word);
            signature.push(format!("{llvm} {name}"));
            lowered.argument = if by_address {
                let (size, align) = self.size_align(ty)?;
                let storage = body.alloca(size, align);
                body.store(&llvm, &name, &storage);
                format!("ptr {storage}")
            } else {
                format!("{kir_ty} {name}")
            };
            return Ok(lowered);
        }
        let (size, align) = self.size_align(ty)?;
        let storage = match (convention, ty) {
            (Convention::Borrow | Convention::BorrowMut, Type::String | Type::Array(_))
                if !(convention == Convention::BorrowMut && *ty == Type::String) =>
            {
                let pair = [
                    crate::cc::Leaf {
                        offset: 0,
                        size: word,
                        class: crate::cc::Class::Integer,
                    },
                    crate::cc::Leaf {
                        offset: word,
                        size: word,
                        class: crate::cc::Class::Integer,
                    },
                ];
                let (data, length) = match allocator.classify(&pair, 2 * word, word, false) {
                    Passing::Direct(_) => {
                        signature.push(format!("{iw} {name}.data"));
                        signature.push(format!("{iw} {name}.length"));
                        let data = body.value();
                        body.line(&format!("{data} = inttoptr {iw} {name}.data to ptr"));
                        (data, format!("{name}.length"))
                    }
                    Passing::Indirect => {
                        signature.push(format!("ptr {name}"));
                        let data = body.load("ptr", &name);
                        let length_at = body.at(&name, word);
                        let length = body.load(&iw, &length_at);
                        (data, length)
                    }
                };
                let copy = body.alloca(size, align);
                body.store("ptr", &data, &copy);
                let at = body.at(&copy, word);
                body.store(&iw, &length, &at);
                let at = body.at(&copy, 2 * word);
                body.store(&iw, &length, &at);
                let at = body.at(&copy, 3 * word);
                body.store("ptr", "null", &at);
                let copier = self.adopter(ty, Mode::Copy)?;
                body.line(&format!("call void {}(ptr {copy})", quote(&copier)));
                if convention == Convention::BorrowMut {
                    let Type::Array(element) = ty else {
                        unreachable!()
                    };
                    if self.adopts(element) {
                        return Err(format!(
                            "RIDL entries cannot pass `borrow mut {}`: its elements own buffers",
                            type_key(ty)
                        ));
                    }
                    let (element_size, element_align) = self.size_align(element)?;
                    lowered.write_back = Some((
                        copy.clone(),
                        data,
                        length,
                        round_up(element_size, element_align),
                    ));
                }
                lowered.destroy = Some((copy.clone(), ty.clone()));
                copy
            }
            (Convention::BorrowMut, _) => {
                allocator.take_address();
                signature.push(format!("ptr {name}"));
                if self.adopts(ty) {
                    let adopter = self.adopter(ty, Mode::Move)?;
                    body.line(&format!("call void {}(ptr {name})", quote(&adopter)));
                    lowered.unique_after = Some((name.clone(), ty.clone()));
                }
                name.clone()
            }
            (Convention::Borrow, _) if self.holds_buffers(ty) && !self.adopts(ty) => {
                // Only boxes, which the callee reads where they are.
                allocator.take_address();
                signature.push(format!("ptr {name}"));
                name.clone()
            }
            (Convention::Borrow, _) if self.holds_buffers(ty) => {
                allocator.take_address();
                signature.push(format!("ptr {name}"));
                let copy = body.alloca(size, align);
                body.line(&format!(
                    "call void @llvm.memcpy.p0.p0.{iw}(ptr {copy}, ptr {name}, {iw} {size}, i1 false)"
                ));
                self.declare(
                    "llvm.memcpy",
                    &format!("declare void @llvm.memcpy.p0.p0.{iw}(ptr, ptr, {iw}, i1)"),
                );
                let copier = self.adopter(ty, Mode::Copy)?;
                body.line(&format!("call void {}(ptr {copy})", quote(&copier)));
                lowered.destroy = Some((copy.clone(), ty.clone()));
                copy
            }
            _ => {
                let leaves = self.leaves(ty)?;
                let storage = match allocator.classify(&leaves, size, word, false) {
                    Passing::Direct(leaves) => {
                        let storage = body.alloca(size, align);
                        for (number, leaf) in leaves.iter().enumerate() {
                            let llvm = leaf_type(leaf);
                            signature.push(format!("{llvm} {name}.{number}"));
                            let at = body.at(&storage, leaf.offset);
                            body.store(&llvm, &format!("{name}.{number}"), &at);
                        }
                        storage
                    }
                    Passing::Indirect => {
                        signature.push(format!("ptr {name}"));
                        name.clone()
                    }
                };
                if convention == Convention::Owned && self.adopts(ty) {
                    let adopter = self.adopter(ty, Mode::Move)?;
                    body.line(&format!("call void {}(ptr {storage})", quote(&adopter)));
                }
                storage
            }
        };
        lowered.argument = if by_address {
            format!("ptr {storage}")
        } else {
            let value = body.load(&kir_ty, &storage);
            format!("{kir_ty} {value}")
        };
        Ok(lowered)
    }

    /// The box holding the value at `receiver`, whose descriptor is
    /// `descriptor`: the value sits at `round_up(2W, align)` (section 21.6).
    pub(super) fn box_of(&self, body: &mut Body, receiver: &str, descriptor: &str) -> String {
        let iw = self.iw();
        let align_at = body.at(descriptor, 8 + self.word);
        let align = body.load(&iw, &align_at);
        let mask = body.value();
        body.line(&format!("{mask} = sub {iw} {align}, 1"));
        let padded = body.value();
        body.line(&format!("{padded} = add {iw} {mask}, {}", 2 * self.word));
        let inverted = body.value();
        body.line(&format!("{inverted} = xor {iw} {mask}, -1"));
        let offset = body.value();
        body.line(&format!("{offset} = and {iw} {padded}, {inverted}"));
        let back = body.value();
        body.line(&format!("{back} = sub {iw} 0, {offset}"));
        let boxed = body.value();
        body.line(&format!(
            "{boxed} = getelementptr inbounds i8, ptr {receiver}, {iw} {back}"
        ));
        boxed
    }

    fn after_call(&mut self, body: &mut Body, lowered: &Lowered) -> Result<(), String> {
        let iw = self.iw();
        if let Some((copy, data, length, stride)) = &lowered.write_back {
            let current = body.load("ptr", copy);
            let bytes = body.value();
            body.line(&format!("{bytes} = mul {iw} {length}, {stride}"));
            body.line(&format!(
                "call void @llvm.memcpy.p0.p0.{iw}(ptr {data}, ptr {current}, {iw} {bytes}, i1 false)"
            ));
        }
        if let Some((value, ty)) = &lowered.unique_after {
            let unique = self.adopter(ty, Mode::Unique)?;
            body.line(&format!("call void {}(ptr {value})", quote(&unique)));
        }
        if let Some((copy, ty)) = &lowered.destroy
            && let Some(destroy) = self.destroy_of(ty)?
        {
            body.line(&format!(
                "call void {}(ptr {copy}, ptr null)",
                quote(&destroy)
            ));
        }
        Ok(())
    }

    pub fn load_leaves(
        &self,
        body: &mut Body,
        storage: &str,
        leaves: &[crate::cc::Leaf],
        result_type: &str,
    ) -> String {
        if leaves.len() == 1 {
            // A scalar result is returned as its own type (`ptr` for a
            // `RawPtr`), which its single leaf does not spell.
            let at = body.at(storage, leaves[0].offset);
            return body.load(result_type, &at);
        }
        let mut aggregate = "poison".to_owned();
        for (number, leaf) in leaves.iter().enumerate() {
            let at = body.at(storage, leaf.offset);
            let value = body.load(&leaf_type(leaf), &at);
            let next = body.value();
            body.line(&format!(
                "{next} = insertvalue {result_type} {aggregate}, {} {value}, {number}",
                leaf_type(leaf)
            ));
            aggregate = next;
        }
        aggregate
    }

    /// After a call returning `Result<T, E>`: returns `T`, or boxes `E` into
    /// the error register (section 22.5).
    #[allow(clippy::too_many_arguments)]
    fn unpack_failure(
        &mut self,
        body: &mut Body,
        storage: &str,
        whole: &Type,
        err: &Type,
        ok: Option<&Type>,
        passing: Option<&Passing>,
        result_type: &str,
    ) -> Result<(), String> {
        let iw = self.iw();
        let shape = self
            .engine
            .enum_shape(whole)
            .map_err(|error| format!("{error:?}"))?
            .ok_or("a failing result is not an enum")?;
        let tag_type = format!("i{}", shape.tag_size * 8);
        let tag = body.load(&tag_type, storage);
        let failed = body.value();
        body.line(&format!("{failed} = icmp ne {tag_type} {tag}, 0"));
        body.line(&format!("br i1 {failed}, label %failed, label %succeeded"));
        body.block("succeeded");
        match (ok, passing) {
            (Some(ok), Some(Passing::Indirect)) => {
                let (size, _) = self.size_align(ok)?;
                let at = body.at(storage, shape.cases[0][0].1);
                body.line(&format!(
                    "call void @llvm.memcpy.p0.p0.{iw}(ptr %result, ptr {at}, {iw} {size}, i1 false)"
                ));
                body.line("ret void");
            }
            (Some(_), Some(Passing::Direct(leaves))) => {
                let at = body.at(storage, shape.cases[0][0].1);
                let value = self.load_leaves(body, &at, leaves, result_type);
                body.line(&format!("ret {result_type} {value}"));
            }
            _ => body.line("ret void"),
        }
        body.block("failed");
        let (size, align) = self.size_align(err)?;
        let value_offset = round_up(2 * self.word, align);
        let descriptor = self.descriptor(err)?;
        let boxed = body.value();
        body.line(&format!(
            "{boxed} = call ptr @malloc({iw} {})",
            value_offset + size
        ));
        body.store("ptr", &quote(&descriptor), &boxed);
        let at = body.at(&boxed, self.word);
        body.store("ptr", "@free", &at);
        let into = body.at(&boxed, value_offset);
        let from = body.at(storage, shape.cases[1][0].1);
        body.line(&format!(
            "call void @llvm.memcpy.p0.p0.{iw}(ptr {into}, ptr {from}, {iw} {size}, i1 false)"
        ));
        self.declare(
            "llvm.memcpy",
            &format!("declare void @llvm.memcpy.p0.p0.{iw}(ptr, ptr, {iw}, i1)"),
        );
        body.store("ptr", &boxed, "%error");
        if result_type == "void" {
            body.line("ret void");
        } else {
            body.line(&format!("ret {result_type} poison"));
        }
        Ok(())
    }

    /// The metadata section and the constructor registering it.
    fn metadata(&mut self, module: &Module, target: &str) {
        let metadata =
            crate::meta::Metadata::for_module(module, Vec::new(), Vec::new(), Vec::new()).encode();
        let section = if target.contains("apple") || target.contains("darwin") {
            "__DATA,__ridlmeta"
        } else if target.contains("windows") {
            ".ridlmt"
        } else {
            ".ridlmeta"
        };
        let bytes = metadata.iter().fold(String::new(), |mut bytes, byte| {
            let _ = write!(bytes, "\\{byte:02X}");
            bytes
        });
        let length = metadata.len();
        let iw = self.iw();
        let _ = writeln!(
            self.globals,
            "@ridl.metadata = private constant [{length} x i8] c\"{bytes}\", section \"{section}\", align 1"
        );
        let _ = writeln!(
            self.globals,
            "@llvm.used = appending global [1 x ptr] [ptr @ridl.metadata], section \"llvm.metadata\""
        );
        let _ = writeln!(
            self.globals,
            "@llvm.global_ctors = appending global [1 x {{ i32, ptr, ptr }}] [{{ i32, ptr, ptr }} {{ i32 65535, ptr @ridl.register, ptr null }}]"
        );
        self.declare(
            "klfrt_ridl_register",
            &format!("declare extern_weak void @klfrt_ridl_register(ptr, {iw})"),
        );
        let _ = write!(
            self.functions,
            "define private void @ridl.register() {{\nentry:\n  %linked = icmp ne ptr @klfrt_ridl_register, null\n  br i1 %linked, label %register, label %done\nregister:\n  call void @klfrt_ridl_register(ptr @ridl.metadata, {iw} {length})\n  br label %done\ndone:\n  ret void\n}}\n\n"
        );
    }
}
