//! Adapters through which a program calls another module's `ridlcc`
//! entries: each defines the symbol the program calls, takes its values in
//! the C view, and calls the entry. Values the entry hands back hold buffers
//! of the provider's allocator; a frontend that adopts takes them over before
//! its code sees them, any other holds them as RIDL buffers.

use super::{
    Body, CView, Ir, Mode, leaf_type, parameters, path, quote, round_up, scalar_type, split_result,
    symbol,
};
use crate::cc::{Allocator, Class, Leaf, Passing, Registers, classify_result};
use crate::module::{Convention, ExtendBody, FunctionDecl, ItemKind, Module};
use crate::types::{Scalar, Type};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The LLVM IR of the adapters for the functions of `module` that `callers`
/// names: an export's path to the symbol the adapter defines. `adopt` takes
/// returned values over through the frontend's buffer functions, which
/// `defined` names.
///
/// # Errors
/// Returns a message when a type has no layout.
pub fn adapters(
    module: &Module,
    callers: &BTreeMap<String, String>,
    target: &str,
    defined: &dyn Fn(&str) -> String,
    adopt: bool,
) -> Result<String, String> {
    let registry = module.registry();
    let destroys = BTreeMap::new();
    let mut ir = Ir::new(&registry, target, &module.name, &destroys, defined, adopt);
    for item in &module.items {
        match &item.kind {
            ItemKind::Function(function) => ir.adapter(callers, target, None, function)?,
            ItemKind::Extend(extend) => {
                if let ExtendBody::Methods(methods) = &extend.body {
                    for method in methods {
                        ir.adapter(callers, target, Some(&extend.target), method)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(ir.finish())
}

/// What an adapter calls.
pub enum Callee {
    /// The exported `ridlcc` entry of this symbol.
    Entry(String),
    /// The method at this index of the witness table of the trait object the
    /// receiver is (sections 21.7, 22.6 and 23.2).
    Witness(usize),
}

/// A trait object opened for a witness call.
struct Opened {
    receiver: String,
    descriptor: String,
    table: String,
    entry: String,
}

/// One argument of the `ridlcc` call: its declared type and its value.
struct Argument {
    ty: String,
    value: String,
}

impl Ir<'_> {
    fn adapter(
        &mut self,
        callers: &BTreeMap<String, String>,
        target: &str,
        owner: Option<&Type>,
        function: &FunctionDecl,
    ) -> Result<(), String> {
        let path = path(owner, function);
        let native = symbol(self.module_name, owner, function);
        let Some(caller) = callers.get(&path) else {
            return Ok(());
        };
        self.define_adapter(target, caller, &Callee::Entry(native), owner, function)
    }

    /// Defines `caller`, which takes `function`'s values in the C view and
    /// calls `callee` with them in `ridlcc`.
    pub(super) fn define_adapter(
        &mut self,
        target: &str,
        caller: &str,
        reached: &Callee,
        owner: Option<&Type>,
        function: &FunctionDecl,
    ) -> Result<(), String> {
        let params = parameters(owner, function);
        let (ok, failure) = split_result(function.result.as_ref());
        let view = CView::of(&params, function.result.as_ref(), self.word);
        let has_sret = view.result_address;
        let word = self.word;
        let iw = self.iw();
        let mut allocator = Allocator::new(Registers::for_target(target));
        let mut body = Body::new();
        let kir_types = view.parameters.clone();
        let incoming: Vec<String> = (0..kir_types.len())
            .map(|index| format!("%a{index}"))
            .collect();
        let first = usize::from(has_sret);
        let result_leaves = match &ok {
            Some(ty) => Some(self.leaves(ty)?),
            None => None,
        };
        let passing = result_leaves.as_deref().map(classify_result);
        let result_type = match (&ok, &passing) {
            (Some(Type::Scalar(scalar)), _) => scalar_type(*scalar, word),
            (Some(_), Some(Passing::Direct(leaves))) if leaves.len() == 1 => leaf_type(&leaves[0]),
            (Some(_), Some(Passing::Direct(leaves))) => format!(
                "{{ {} }}",
                leaves.iter().map(leaf_type).collect::<Vec<_>>().join(", ")
            ),
            _ => "void".to_owned(),
        };
        // Where `T` of a failing function lands: the payload of the Result
        // the frontend's caller passed.
        let shape = match &failure {
            Some(err) => {
                let whole = Type::Result(
                    Box::new(ok.clone().unwrap_or(Type::Tuple(Vec::new()))),
                    Box::new(err.clone()),
                );
                Some(
                    self.engine
                        .enum_shape(&whole)
                        .map_err(|error| format!("{error:?}"))?
                        .ok_or("a failing result is not an enum")?,
                )
            }
            None => None,
        };
        let result_at = match (&shape, has_sret) {
            (Some(shape), true) => shape.cases[0]
                .first()
                .map(|(_, offset)| body.at("%a0", *offset)),
            (None, true) => Some("%a0".to_owned()),
            (_, false) => None,
        };
        let mut arguments = Vec::new();
        if matches!(passing, Some(Passing::Indirect)) {
            let (size, _) = self.size_align(ok.as_ref().expect("an indirect result has a type"))?;
            let at = result_at
                .clone()
                .ok_or("an indirect RIDL result without a result address")?;
            arguments.push(Argument {
                ty: format!("ptr noalias sret([{size} x i8])"),
                value: at,
            });
            allocator.take_address();
        }
        let mut adopt_after = Vec::new();
        let mut witness = None;
        for (index, (convention, ty)) in params.iter().enumerate() {
            let value = incoming[index + first].clone();
            let kir_ty = &kir_types[index + first];
            if let (Callee::Witness(slot), 0) = (reached, index) {
                allocator.take_address();
                let opened = self.open_trait_object(&mut body, &value, *slot, *convention);
                arguments.push(Argument {
                    ty: "ptr".to_owned(),
                    value: opened.receiver.clone(),
                });
                witness = Some(opened);
                continue;
            }
            self.argument(
                &mut body,
                &mut arguments,
                &mut allocator,
                &mut adopt_after,
                *convention,
                ty,
                &value,
                kir_ty,
            )?;
        }
        if let Some(opened) = &witness {
            arguments.push(Argument {
                ty: "ptr".to_owned(),
                value: opened.descriptor.clone(),
            });
            arguments.push(Argument {
                ty: "ptr".to_owned(),
                value: opened.table.clone(),
            });
        }
        if failure.is_some() {
            let slot = body.value();
            body.line(&format!("{slot} = alloca swifterror ptr, align {word}"));
            body.line(&format!("store ptr null, ptr {slot}, align {word}"));
            arguments.push(Argument {
                ty: "ptr swifterror".to_owned(),
                value: slot,
            });
        }
        let declared: Vec<String> = arguments
            .iter()
            .map(|argument| argument.ty.clone())
            .collect();
        let target_value = match (reached, &witness) {
            (Callee::Entry(native), _) => {
                self.declare(
                    native,
                    &format!(
                        "declare swiftcc {result_type} {}({})",
                        quote(native),
                        declared.join(", ")
                    ),
                );
                quote(native)
            }
            (Callee::Witness(_), Some(opened)) => opened.entry.clone(),
            (Callee::Witness(_), None) => {
                return Err(format!("`{caller}` calls a witness without a receiver"));
            }
        };
        let call = format!(
            "call swiftcc {result_type} {target_value}({})",
            arguments
                .iter()
                .map(|argument| format!("{} {}", argument.ty, argument.value))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let returned = if result_type == "void" {
            body.line(&call);
            None
        } else {
            let value = body.value();
            body.line(&format!("{value} = {call}"));
            Some(value)
        };
        for (address, ty) in &adopt_after {
            let adopter = self.adopter(ty, Mode::Move)?;
            body.line(&format!("call void {}(ptr {address})", quote(&adopter)));
        }
        let kir_result = view.result.clone();
        match (&failure, shape) {
            (Some(err), Some(shape)) => {
                let error_slot = arguments
                    .last()
                    .map(|argument| argument.value.clone())
                    .unwrap_or_default();
                let boxed = body.load("ptr", &error_slot);
                let failed = body.value();
                body.line(&format!("{failed} = icmp ne ptr {boxed}, null"));
                body.line(&format!("br i1 {failed}, label %failed, label %succeeded"));
                let tag_type = format!("i{}", shape.tag_size * 8);
                body.block("succeeded");
                body.store(&tag_type, "0", "%a0");
                if let (Some(ok), Some(at)) = (&ok, &result_at) {
                    if let (Some(Passing::Direct(leaves)), Some(value)) = (&passing, &returned) {
                        Self::store_leaves(&mut body, at, leaves, value, &result_type);
                    }
                    if self.adopt && self.adopts(ok) {
                        let adopter = self.adopter(ok, Mode::Move)?;
                        body.line(&format!("call void {}(ptr {at})", quote(&adopter)));
                    }
                }
                body.line("ret void");
                body.block("failed");
                body.store(&tag_type, "1", "%a0");
                let (size, align) = self.size_align(err)?;
                let from = body.at(&boxed, round_up(2 * word, align));
                let into = body.at("%a0", shape.cases[1][0].1);
                body.line(&format!(
                    "call void @llvm.memcpy.p0.p0.{iw}(ptr {into}, ptr {from}, {iw} {size}, i1 false)"
                ));
                let free_at = body.at(&boxed, word);
                let free = body.load("ptr", &free_at);
                body.line(&format!("call void {free}(ptr {boxed})"));
                if self.adopt && self.adopts(err) {
                    let adopter = self.adopter(err, Mode::Move)?;
                    body.line(&format!("call void {}(ptr {into})", quote(&adopter)));
                }
                body.line("ret void");
            }
            _ => match (&ok, &passing, returned) {
                (None, _, _) => body.line("ret void"),
                (Some(ok), Some(Passing::Indirect), _) => {
                    if self.adopt && self.adopts(ok) {
                        let adopter = self.adopter(ok, Mode::Move)?;
                        body.line(&format!("call void {}(ptr %a0)", quote(&adopter)));
                    }
                    body.line("ret void");
                }
                (Some(ok), Some(Passing::Direct(leaves)), Some(value)) if has_sret => {
                    Self::store_leaves(&mut body, "%a0", leaves, &value, &result_type);
                    if self.adopt && self.adopts(ok) {
                        let adopter = self.adopter(ok, Mode::Move)?;
                        body.line(&format!("call void {}(ptr %a0)", quote(&adopter)));
                    }
                    body.line("ret void");
                }
                (Some(_), _, Some(value)) => body.line(&format!("ret {kir_result} {value}")),
                (Some(_), _, None) => body.line("ret void"),
            },
        }
        let parameters: Vec<String> = kir_types
            .iter()
            .zip(&incoming)
            .map(|(ty, name)| format!("{ty} {name}"))
            .collect();
        let _ = write!(
            self.functions,
            "define weak_odr {kir_result} {}({}) {{\nentry:\n{}}}\n\n",
            quote(caller),
            parameters.join(", "),
            body.text
        );
        Ok(())
    }

    /// Lowers one argument the frontend passed as `value` (typed `kir_ty`)
    /// into the `ridlcc` form of a `convention` parameter of type `ty`.
    #[allow(clippy::too_many_arguments)]
    fn argument(
        &mut self,
        body: &mut Body,
        arguments: &mut Vec<Argument>,
        allocator: &mut Allocator,
        adopt_after: &mut Vec<(String, Type)>,
        convention: Convention,
        ty: &Type,
        value: &str,
        kir_ty: &str,
    ) -> Result<(), String> {
        let word = self.word;
        let iw = self.iw();
        let by_address = kir_ty == "ptr" && !matches!(ty, Type::Scalar(Scalar::RawPtr));
        if let Type::Scalar(scalar) = ty {
            if convention == Convention::BorrowMut {
                allocator.take_address();
                arguments.push(Argument {
                    ty: "ptr".to_owned(),
                    value: value.to_owned(),
                });
                return Ok(());
            }
            let leaves = self.leaves(ty)?;
            allocator.classify(&leaves, self.size_align(ty)?.0, word, true);
            let llvm = scalar_type(*scalar, word);
            let value = if by_address {
                body.load(&llvm, value)
            } else {
                value.to_owned()
            };
            arguments.push(Argument { ty: llvm, value });
            return Ok(());
        }
        let pair = matches!(
            (convention, ty),
            (Convention::Borrow, Type::String | Type::Array(_))
                | (Convention::BorrowMut, Type::Array(_))
        );
        if pair {
            let data = body.load("ptr", value);
            let length_at = body.at(value, word);
            let length = body.load(&iw, &length_at);
            let leaves = [
                Leaf {
                    offset: 0,
                    size: word,
                    class: Class::Integer,
                },
                Leaf {
                    offset: word,
                    size: word,
                    class: Class::Integer,
                },
            ];
            match allocator.classify(&leaves, 2 * word, word, false) {
                Passing::Direct(_) => {
                    let address = body.value();
                    body.line(&format!("{address} = ptrtoint ptr {data} to {iw}"));
                    arguments.push(Argument {
                        ty: iw.clone(),
                        value: address,
                    });
                    arguments.push(Argument {
                        ty: iw,
                        value: length,
                    });
                }
                Passing::Indirect => {
                    let both = body.alloca(2 * word, word);
                    body.store("ptr", &data, &both);
                    let at = body.at(&both, word);
                    body.store(&iw, &length, &at);
                    arguments.push(Argument {
                        ty: "ptr".to_owned(),
                        value: both,
                    });
                }
            }
            return Ok(());
        }
        // A value handed over, or lent to be changed and taken over in
        // place, leaves holding only buffers the frontend has alone
        // (section 21.4).
        let handed = convention != Convention::Borrow && self.adopt && self.adopts(ty);
        if handed && by_address {
            let unique = self.adopter(ty, Mode::Unique)?;
            body.line(&format!("call void {}(ptr {value})", quote(&unique)));
        }
        if convention == Convention::BorrowMut
            || (convention == Convention::Borrow && self.holds_buffers(ty))
        {
            allocator.take_address();
            arguments.push(Argument {
                ty: "ptr".to_owned(),
                value: value.to_owned(),
            });
            if self.adopt && convention == Convention::BorrowMut && self.adopts(ty) {
                adopt_after.push((value.to_owned(), ty.clone()));
            }
            return Ok(());
        }
        let (size, _) = self.size_align(ty)?;
        let leaves = self.leaves(ty)?;
        match allocator.classify(&leaves, size, word, false) {
            Passing::Direct(leaves) => {
                for leaf in &leaves {
                    let llvm = leaf_type(leaf);
                    let at = body.at(value, leaf.offset);
                    let loaded = body.load(&llvm, &at);
                    arguments.push(Argument {
                        ty: llvm,
                        value: loaded,
                    });
                }
            }
            Passing::Indirect => arguments.push(Argument {
                ty: "ptr".to_owned(),
                value: value.to_owned(),
            }),
        }
        Ok(())
    }

    /// Reads the trait object at `object` for a call of the method in
    /// witness-table slot `slot`. A receiver the method changes or takes over
    /// is first made the box's only holder (`BOX_UNIQUE`), so no other holder
    /// of a shared box sees it change; a box nothing else holds, which is
    /// every box a value tree passes down, is used as it is, with no copy. A
    /// method taking `self` takes over the whole box, the value and the
    /// storage around it.
    fn open_trait_object(
        &mut self,
        body: &mut Body,
        object: &str,
        slot: usize,
        convention: Convention,
    ) -> Opened {
        let word = self.word;
        let iw = self.iw();
        let mut boxed = body.load("ptr", object);
        if convention != Convention::Borrow {
            let unique = quote(&(self.defined)(super::BOX_UNIQUE));
            self.declare(super::BOX_UNIQUE, &format!("declare ptr {unique}(ptr)"));
            let owned = body.value();
            body.line(&format!("{owned} = call ptr {unique}(ptr {boxed})"));
            body.store("ptr", &owned, object);
            boxed = owned;
        }
        let table_at = body.at(object, word);
        let table = body.load("ptr", &table_at);
        let descriptor = body.load("ptr", &boxed);
        let align_at = body.at(&descriptor, 8 + word);
        let align = body.load(&iw, &align_at);
        let mask = body.value();
        body.line(&format!("{mask} = sub {iw} {align}, 1"));
        let padded = body.value();
        body.line(&format!("{padded} = add {iw} {mask}, {}", 2 * word));
        let inverted = body.value();
        body.line(&format!("{inverted} = xor {iw} {mask}, -1"));
        let offset = body.value();
        body.line(&format!("{offset} = and {iw} {padded}, {inverted}"));
        let receiver = body.value();
        body.line(&format!(
            "{receiver} = getelementptr inbounds i8, ptr {boxed}, {iw} {offset}"
        ));
        let entry_at = body.at(&table, 8 + word + slot as u64 * word);
        let entry = body.load("ptr", &entry_at);
        Opened {
            receiver,
            descriptor,
            table,
            entry,
        }
    }

    /// Stores the leaves of a direct `ridlcc` result at `address`.
    fn store_leaves(
        body: &mut Body,
        address: &str,
        leaves: &[Leaf],
        value: &str,
        result_type: &str,
    ) {
        if leaves.len() == 1 {
            // A scalar result arrives as its own type (`ptr` for a
            // `RawPtr`), which its single leaf does not spell.
            let at = body.at(address, leaves[0].offset);
            body.store(result_type, value, &at);
            return;
        }
        for (number, leaf) in leaves.iter().enumerate() {
            let part = body.value();
            body.line(&format!(
                "{part} = extractvalue {result_type} {value}, {number}"
            ));
            let at = body.at(address, leaf.offset);
            body.store(&leaf_type(leaf), &part, &at);
        }
    }
}
