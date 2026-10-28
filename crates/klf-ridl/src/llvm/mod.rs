//! LLVM IR for the RIDL boundary of a module (sections 21 to 24): the
//! `ridlcc` entries a library exports (`entries`), the adapters through
//! which a program calls another module's entries (`adapters`), and what both
//! share: type descriptors, the module metadata, and the functions that take
//! over values holding another allocator's buffers.
//!
//! Both sides call or are called by a frontend function in its C view
//! ([`CView`]): scalars by value, every other value and every `borrow mut`
//! by address, and a result that is not a scalar written through a leading
//! address. LLVM's `swiftcc` carries `ridlcc`; the classification of section
//! 22.3 is done here, so LLVM is handed only scalars and pointers.

mod adapters;
mod entries;
mod witnesses;

pub use adapters::adapters;
pub use entries::{Receiving, entries};
pub use witnesses::{
    Conformance, ImportedTable, WitnessCall, WitnessDefault, WitnessSet, witnesses,
};

use crate::cc::leaves;
use crate::descriptor::flags;
use crate::emit::type_key;
use crate::layout::LayoutEngine;
use crate::module::{Convention, FunctionDecl, SelfParam};
use crate::types::{Decl, Registry, Scalar, Type};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A frontend's accessor for the `BufferOps` of its own buffers.
pub const BUFFER_OPS: &str = "__klf_ridl_buffer_ops";
/// A frontend's allocator of a buffer of a byte count.
pub const BUFFER_ALLOCATE: &str = "__klf_ridl_buffer_allocate";
/// A frontend's test of whether one of its own buffers, by its data, has no
/// other holder: a frontend whose buffers are shared until written answers
/// no for one it shares, and every other frontend answers yes.
pub const BUFFER_UNIQUE: &str = "__klf_ridl_buffer_unique";
/// A frontend's function making it the only holder of a RIDL box: the box
/// itself when nothing else holds it, otherwise a copy made through the
/// box's descriptor, with the shared box released.
pub const BOX_UNIQUE: &str = "__klf_ridl_box_unique";
/// A frontend's copy of a RIDL box it is handed a second holder of (section
/// 21.7): the box it answers is the copy's to destroy. A frontend whose boxes
/// count their holders answers the same box, held once more.
pub const BOX_COPY: &str = "__klf_ridl_box_copy";

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub enum Mode {
    /// Take the value over in place, replacing foreign buffers.
    Move,
    /// Make an independent copy whose buffers are all the frontend's.
    Copy,
    /// Make a value about to be handed over the frontend's alone: an owned
    /// buffer belongs to its receiver with its elements (section 21.4), so
    /// one the frontend shares is copied, as a copy-on-write array is when
    /// it is not uniquely owned (section 30), and its elements with it.
    Unique,
}

/// The LLVM IR being written for one module's RIDL boundary.
pub struct Ir<'a> {
    pub engine: LayoutEngine<'a>,
    pub registry: &'a Registry,
    pub word: u64,
    pub module_name: &'a str,
    destroys: &'a BTreeMap<String, String>,
    /// Whether values the frontend receives are taken over into its own
    /// buffers. A frontend that holds RIDL buffers as they are does not.
    pub adopt: bool,
    /// The object symbol of a function the frontend defines.
    pub defined: &'a dyn Fn(&str) -> String,
    declarations: BTreeMap<String, String>,
    pub functions: String,
    globals: String,
    adopters: BTreeMap<(String, Mode), String>,
    descriptors: BTreeMap<String, String>,
    /// The linkage of a declared type's `D` symbol: exported by the module
    /// declaring the type, unless another object of the same program
    /// already does.
    pub descriptor_linkage: &'static str,
}

/// One function body being written, with its own value numbering.
pub struct Body {
    pub text: String,
    next: usize,
}

impl<'a> Ir<'a> {
    pub fn new(
        registry: &'a Registry,
        target: &str,
        module_name: &'a str,
        destroys: &'a BTreeMap<String, String>,
        defined: &'a dyn Fn(&str) -> String,
        adopt: bool,
    ) -> Self {
        let word = if target.starts_with("wasm32") { 4 } else { 8 };
        let mut ir = Ir {
            engine: LayoutEngine::new(registry, word),
            registry,
            word,
            module_name,
            destroys,
            adopt,
            defined,
            declarations: BTreeMap::new(),
            functions: String::new(),
            globals: String::new(),
            adopters: BTreeMap::new(),
            descriptors: BTreeMap::new(),
            descriptor_linkage: "",
        };
        if adopt {
            let ops = quote(&defined(BUFFER_OPS));
            let allocate = quote(&defined(BUFFER_ALLOCATE));
            let unique = quote(&defined(BUFFER_UNIQUE));
            ir.declare(BUFFER_OPS, &format!("declare ptr {ops}()"));
            ir.declare(BUFFER_ALLOCATE, &format!("declare ptr {allocate}(i64)"));
            ir.declare(BUFFER_UNIQUE, &format!("declare i1 {unique}(ptr)"));
        }
        ir.declare("malloc", &format!("declare ptr @malloc({})", ir.iw()));
        ir.declare("free", "declare void @free(ptr)");
        ir.declare(
            "llvm.memcpy",
            &format!(
                "declare void @llvm.memcpy.p0.p0.{0}(ptr, ptr, {0}, i1)",
                ir.iw()
            ),
        );
        ir
    }

    /// The whole IR module.
    #[must_use]
    pub fn finish(self) -> String {
        let mut out = String::new();
        for line in self.declarations.values() {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(&self.globals);
        out.push_str(&self.functions);
        out
    }
}

impl Body {
    #[must_use]
    pub fn new() -> Self {
        Self {
            text: String::new(),
            next: 0,
        }
    }

    pub fn value(&mut self) -> String {
        self.next += 1;
        format!("%v{}", self.next)
    }

    pub fn label(&mut self, stem: &str) -> String {
        self.next += 1;
        format!("{stem}{}", self.next)
    }

    pub fn line(&mut self, line: &str) {
        self.text.push_str("  ");
        self.text.push_str(line);
        self.text.push('\n');
    }

    pub fn block(&mut self, label: &str) {
        let _ = writeln!(self.text, "{label}:");
    }

    /// The address `offset` bytes past `base`.
    pub fn at(&mut self, base: &str, offset: u64) -> String {
        if offset == 0 {
            return base.to_owned();
        }
        let value = self.value();
        self.line(&format!(
            "{value} = getelementptr inbounds i8, ptr {base}, i64 {offset}"
        ));
        value
    }

    pub fn load(&mut self, ty: &str, address: &str) -> String {
        let value = self.value();
        self.line(&format!("{value} = load {ty}, ptr {address}, align 1"));
        value
    }

    pub fn store(&mut self, ty: &str, value: &str, address: &str) {
        self.line(&format!("store {ty} {value}, ptr {address}, align 1"));
    }

    pub fn alloca(&mut self, size: u64, align: u64) -> String {
        let value = self.value();
        self.line(&format!(
            "{value} = alloca [{} x i8], align {}",
            size.max(1),
            align.max(1)
        ));
        value
    }
}

impl Default for Body {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn quote(symbol: &str) -> String {
    format!("@\"{symbol}\"")
}

#[must_use]
pub fn scalar_type(scalar: Scalar, word: u64) -> String {
    match scalar {
        Scalar::I8 | Scalar::U8 | Scalar::Bool => "i8".to_owned(),
        Scalar::I16 | Scalar::U16 => "i16".to_owned(),
        Scalar::I32 | Scalar::U32 => "i32".to_owned(),
        Scalar::I64 | Scalar::U64 => "i64".to_owned(),
        Scalar::I128 | Scalar::U128 => "i128".to_owned(),
        Scalar::F32 => "float".to_owned(),
        Scalar::F64 => "double".to_owned(),
        Scalar::ISize | Scalar::USize => format!("i{}", word * 8),
        Scalar::RawPtr => "ptr".to_owned(),
        Scalar::Void => "void".to_owned(),
    }
}

#[must_use]
pub fn leaf_type(leaf: &crate::cc::Leaf) -> String {
    match leaf.class {
        crate::cc::Class::Float if leaf.size == 4 => "float".to_owned(),
        crate::cc::Class::Float => "double".to_owned(),
        crate::cc::Class::Integer => format!("i{}", leaf.size * 8),
    }
}

#[must_use]
pub fn round_up(value: u64, align: u64) -> u64 {
    value.div_ceil(align.max(1)) * align.max(1)
}

impl Ir<'_> {
    #[must_use]
    pub fn iw(&self) -> String {
        format!("i{}", self.word * 8)
    }

    pub fn declare(&mut self, key: &str, line: &str) {
        self.declarations
            .entry(key.to_owned())
            .or_insert_with(|| line.to_owned());
    }

    /// The size and alignment of `ty`.
    ///
    /// # Errors
    /// Returns the layout engine's error when `ty` has no layout.
    pub fn size_align(&self, ty: &Type) -> Result<(u64, u64), String> {
        let layout = self
            .engine
            .layout_of(ty)
            .map_err(|error| format!("{error:?}"))?;
        Ok((layout.size, layout.align))
    }

    /// The scalar leaves `ty` flattens to (section 22.3).
    ///
    /// # Errors
    /// Returns the layout engine's error when `ty` has no layout.
    pub fn leaves(&self, ty: &Type) -> Result<Vec<crate::cc::Leaf>, String> {
        leaves(&self.engine, ty).map_err(|error| format!("{error:?}"))
    }

    /// The fields of a struct with their types and offsets.
    ///
    /// # Errors
    /// Returns an error when `name` is not a known struct or has no layout.
    pub fn fields(&self, name: &str, args: &[Type]) -> Result<Vec<(Type, u64)>, String> {
        let Some(Decl::Struct(decl)) = self.registry.get(name) else {
            return Err(format!("`{name}` is not a struct"));
        };
        let subst = decl
            .generics
            .iter()
            .cloned()
            .zip(args.iter().cloned())
            .collect();
        let types: Vec<Type> = decl
            .fields
            .iter()
            .flatten()
            .map(|field| field.ty.substitute(&subst))
            .collect();
        let (_, placements) = self
            .engine
            .aggregate(&types)
            .map_err(|error| format!("{error:?}"))?;
        Ok(types
            .into_iter()
            .zip(placements)
            .map(|(ty, placement)| (ty, placement.offset))
            .collect())
    }

    /// Whether a value of `ty` can hold a buffer or a box, and so needs
    /// destroying.
    #[must_use]
    pub fn holds_buffers(&self, ty: &Type) -> bool {
        self.holds(ty, true)
    }

    /// Whether a value of `ty` can hold a buffer another allocator made, and
    /// so needs adopting or copying. A closure's context and a trait object's
    /// value are RIDL boxes (sections 21.6 to 21.8): each carries the
    /// descriptor that copies and destroys what it holds and the `free` of
    /// the allocator that made it, so it is taken over as it is.
    #[must_use]
    pub fn adopts(&self, ty: &Type) -> bool {
        self.holds(ty, false)
    }

    /// Whether `mode`'s adopter has anything to do for a value of `ty`. A
    /// copy also reaches the boxes, since the copy needs holders of its own.
    fn reaches(&self, ty: &Type, mode: Mode) -> bool {
        self.holds(ty, mode == Mode::Copy)
    }

    fn holds(&self, ty: &Type, boxes: bool) -> bool {
        match ty {
            Type::Scalar(_) | Type::Generic(_) => false,
            Type::String | Type::Array(_) => true,
            Type::Function { .. } | Type::BorrowFunction { .. } | Type::TraitObject(_) => boxes,
            Type::Optional(inner) => self.holds(inner, boxes),
            Type::Result(ok, err) => self.holds(ok, boxes) || self.holds(err, boxes),
            Type::Tuple(items) => items.iter().any(|item| self.holds(item, boxes)),
            Type::Nominal { name, args } => match self.registry.get(name) {
                Some(Decl::Struct(decl)) => {
                    let subst = decl
                        .generics
                        .iter()
                        .cloned()
                        .zip(args.iter().cloned())
                        .collect();
                    decl.fields
                        .iter()
                        .flatten()
                        .any(|field| self.holds(&field.ty.substitute(&subst), boxes))
                }
                Some(Decl::Enum(decl)) => {
                    let subst = decl
                        .generics
                        .iter()
                        .cloned()
                        .zip(args.iter().cloned())
                        .collect();
                    decl.cases.iter().any(|case| {
                        case.payload
                            .iter()
                            .any(|payload| self.holds(&payload.substitute(&subst), boxes))
                    })
                }
                None => false,
            },
        }
    }

    /// The destroy function the frontend exports for `ty`, if it has one.
    ///
    /// # Errors
    /// Returns an error when `ty` holds buffers but the RIDL surface exports
    /// no destroy function for it.
    pub fn destroy_of(&mut self, ty: &Type) -> Result<Option<String>, String> {
        if !self.holds_buffers(ty) {
            return Ok(None);
        }
        let key = type_key(ty);
        let Some(symbol) = self.destroys.get(&key).filter(|symbol| !symbol.is_empty()) else {
            return Err(format!(
                "the RIDL surface has no destroy function for `{key}`"
            ));
        };
        let symbol = (self.defined)(symbol);
        self.declare(
            &symbol,
            &format!("declare void {}(ptr, ptr)", quote(&symbol)),
        );
        Ok(Some(symbol))
    }

    /// The descriptor of `ty` (section 23.1): exported under its `D` symbol
    /// for a declared type, private for a built-in one.
    ///
    /// # Errors
    /// Returns the layout engine's error when `ty` has no layout.
    pub fn descriptor(&mut self, ty: &Type) -> Result<String, String> {
        let key = type_key(ty);
        if let Some(name) = self.descriptors.get(&key) {
            return Ok(name.clone());
        }
        let (name, linkage) = match ty {
            Type::Nominal { name, args } if args.is_empty() => (
                crate::mangle::descriptor_symbol(self.module_name, name),
                self.descriptor_linkage,
            ),
            _ => (format!("ridl.descriptor.{key}"), "private "),
        };
        self.descriptors.insert(key, name.clone());
        let (size, align) = self.size_align(ty)?;
        let destroy = self.destroy_of(ty)?;
        let mut bits = flags::RELOCATABLE;
        if destroy.is_none() {
            bits |= flags::COPYABLE | flags::TRIVIAL_DESTROY;
        }
        let iw = self.iw();
        let destroy = destroy.map_or_else(|| "null".to_owned(), |symbol| quote(&symbol));
        let _ = writeln!(
            self.globals,
            "{} = {linkage}constant {{ i32, i32, {iw}, {iw}, ptr, ptr, ptr }} {{ i32 1, i32 {bits}, {iw} {size}, {iw} {align}, ptr null, ptr null, ptr {destroy} }}, align {}",
            quote(&name),
            self.word
        );
        Ok(name)
    }

    /// The function adopting or copying a value of `ty` at an address.
    ///
    /// # Errors
    /// Returns the layout engine's error when `ty` has no layout.
    pub fn adopter(&mut self, ty: &Type, mode: Mode) -> Result<String, String> {
        let key = (type_key(ty), mode);
        if let Some(name) = self.adopters.get(&key) {
            return Ok(name.clone());
        }
        let name = format!(
            "ridl.{}.{}",
            match mode {
                Mode::Move => "adopt",
                Mode::Copy => "copy",
                Mode::Unique => "unique",
            },
            key.0
        );
        self.adopters.insert(key, name.clone());
        let mut body = Body::new();
        match ty {
            Type::String if mode == Mode::Unique => self.unique_buffer(&mut body, None)?,
            Type::Array(element) if mode == Mode::Unique => {
                self.unique_buffer(&mut body, Some(element))?;
            }
            Type::TraitObject(_) if mode == Mode::Copy => self.copy_box(&mut body, 0),
            Type::Function { .. } if mode == Mode::Copy => self.copy_box(&mut body, self.word),
            Type::BorrowFunction { .. } if mode == Mode::Copy => body.line("ret void"),
            Type::String => self.adopt_buffer(&mut body, None, mode)?,
            Type::Array(element) => self.adopt_buffer(&mut body, Some(element), mode)?,
            Type::Tuple(items) => {
                let (_, placements) = self
                    .engine
                    .aggregate(items)
                    .map_err(|error| format!("{error:?}"))?;
                let parts: Vec<(Type, u64)> = items
                    .iter()
                    .cloned()
                    .zip(placements.into_iter().map(|placement| placement.offset))
                    .collect();
                self.adopt_parts(&mut body, &parts, mode)?;
                body.line("ret void");
            }
            Type::Nominal {
                name: type_name,
                args,
            } if matches!(self.registry.get(type_name), Some(Decl::Struct(_))) => {
                let parts = self.fields(type_name, args)?;
                self.adopt_parts(&mut body, &parts, mode)?;
                body.line("ret void");
            }
            Type::Optional(_) | Type::Result(..) | Type::Nominal { .. } => {
                self.adopt_enum(&mut body, ty, mode)?;
            }
            other => {
                return Err(format!(
                    "RIDL entries cannot take over a value of `{}`",
                    type_key(other)
                ));
            }
        }
        let _ = write!(
            self.functions,
            "define private void {}(ptr %value) {{\nentry:\n{}}}\n\n",
            quote(&name),
            body.text
        );
        Ok(name)
    }

    fn adopt_parts(
        &mut self,
        body: &mut Body,
        parts: &[(Type, u64)],
        mode: Mode,
    ) -> Result<(), String> {
        for (ty, offset) in parts {
            if self.reaches(ty, mode) {
                let adopter = self.adopter(ty, mode)?;
                let address = body.at("%value", *offset);
                body.line(&format!("call void {}(ptr {address})", quote(&adopter)));
            }
        }
        Ok(())
    }

    fn adopt_enum(&mut self, body: &mut Body, ty: &Type, mode: Mode) -> Result<(), String> {
        let shape = self
            .engine
            .enum_shape(ty)
            .map_err(|error| format!("{error:?}"))?
            .ok_or_else(|| format!("`{}` is not an enum", type_key(ty)))?;
        if shape.niche {
            // The payload-less case is a zero first word; the other case's
            // one payload starts at offset 0 (section 21.3.4).
            let payload = shape.cases.iter().find(|parts| !parts.is_empty());
            if let Some(parts) = payload.filter(|parts| parts.iter().any(|(ty, _)| self.reaches(ty, mode))) {
                let word = body.load("ptr", "%value");
                let absent = body.value();
                body.line(&format!("{absent} = icmp eq ptr {word}, null"));
                body.line(&format!("br i1 {absent}, label %done, label %present"));
                body.block("present");
                self.adopt_parts(body, parts, mode)?;
                body.line("br label %done");
                body.block("done");
            }
            body.line("ret void");
            return Ok(());
        }
        let tag_type = format!("i{}", shape.tag_size * 8);
        let tag = body.load(&tag_type, "%value");
        let mut cases = Vec::new();
        for (index, payload) in shape.cases.iter().enumerate() {
            if payload.iter().any(|(ty, _)| self.reaches(ty, mode)) {
                cases.push((index, body.label("case")));
            }
        }
        let arms: Vec<String> = cases
            .iter()
            .map(|(index, label)| format!("{tag_type} {index}, label %{label}"))
            .collect();
        body.line(&format!(
            "switch {tag_type} {tag}, label %done [ {} ]",
            arms.join(" ")
        ));
        for (index, label) in cases {
            body.block(&label);
            self.adopt_parts(body, &shape.cases[index], mode)?;
            body.line("br label %done");
        }
        body.block("done");
        body.line("ret void");
        Ok(())
    }

    /// Gives the box at `offset` in `%value` a holder of its own through the
    /// frontend's `BOX_COPY`: the box of an `Any Trait`, or the context of a
    /// closure, which may be null.
    fn copy_box(&mut self, body: &mut Body, offset: u64) {
        let copy = quote(&(self.defined)(BOX_COPY));
        self.declare(BOX_COPY, &format!("declare ptr {copy}(ptr)"));
        let at = body.at("%value", offset);
        let boxed = body.load("ptr", &at);
        let absent = body.value();
        body.line(&format!("{absent} = icmp eq ptr {boxed}, null"));
        body.line(&format!("br i1 {absent}, label %done, label %held"));
        body.block("held");
        let copied = body.value();
        body.line(&format!("{copied} = call ptr {copy}(ptr {boxed})"));
        body.store("ptr", &copied, &at);
        body.line("br label %done");
        body.block("done");
        body.line("ret void");
    }

    /// Adopts or copies the buffer of a `String` or `[T]` at `%value`.
    fn adopt_buffer(
        &mut self,
        body: &mut Body,
        element: Option<&Type>,
        mode: Mode,
    ) -> Result<(), String> {
        let word = self.word;
        let iw = self.iw();
        let (stride, align) = match element {
            Some(element) => {
                let (size, align) = self.size_align(element)?;
                (round_up(size, align), align)
            }
            None => (1, 1),
        };
        let data = body.load("ptr", "%value");
        let length_at = body.at("%value", word);
        let length = body.load(&iw, &length_at);
        let capacity_at = body.at("%value", 2 * word);
        let capacity = body.load(&iw, &capacity_at);
        let ops_at = body.at("%value", 3 * word);
        let ops = body.load("ptr", &ops_at);
        let own_ops = body.value();
        body.line(&format!(
            "{own_ops} = call ptr {}()",
            quote(&(self.defined)(BUFFER_OPS))
        ));
        if mode == Mode::Move {
            let own = body.value();
            body.line(&format!("{own} = icmp eq ptr {ops}, {own_ops}"));
            body.line(&format!("br i1 {own}, label %elements, label %foreign"));
            body.block("foreign");
        } else {
            body.line("br label %foreign");
            body.block("foreign");
        }
        let bytes = body.value();
        body.line(&format!("{bytes} = mul {iw} {length}, {stride}"));
        let bytes64 = if word == 8 {
            bytes.clone()
        } else {
            let wide = body.value();
            body.line(&format!("{wide} = zext {iw} {bytes} to i64"));
            wide
        };
        let fresh = body.value();
        body.line(&format!(
            "{fresh} = call ptr {}(i64 {bytes64})",
            quote(&(self.defined)(BUFFER_ALLOCATE))
        ));
        body.line(&format!(
            "call void @llvm.memcpy.p0.p0.{iw}(ptr {fresh}, ptr {data}, {iw} {bytes}, i1 false)"
        ));
        self.declare(
            "llvm.memcpy",
            &format!("declare void @llvm.memcpy.p0.p0.{iw}(ptr, ptr, {iw}, i1)"),
        );
        if mode == Mode::Move {
            let live = body.value();
            body.line(&format!("{live} = icmp ne ptr {ops}, null"));
            body.line(&format!("br i1 {live}, label %release, label %replace"));
            body.block("release");
            let free_at = body.at(&ops, 8 + word);
            let free = body.load("ptr", &free_at);
            body.line(&format!(
                "call void {free}(ptr {data}, {iw} {capacity}, {iw} {stride}, {iw} {align})"
            ));
            body.line("br label %replace");
        } else {
            body.line("br label %replace");
        }
        body.block("replace");
        body.store("ptr", &fresh, "%value");
        body.store(&iw, &length, &capacity_at);
        body.store("ptr", &own_ops, &ops_at);
        body.line("br label %elements");
        body.block("elements");
        match element.filter(|element| self.reaches(element, mode)) {
            Some(element) => {
                let adopter = self.adopter(element, mode)?;
                body.line("br label %loop");
                body.block("loop");
                body.line(&format!(
                    "%index = phi {iw} [ 0, %elements ], [ %following, %each ]"
                ));
                body.line(&format!("%finished = icmp eq {iw} %index, {length}"));
                body.line("br i1 %finished, label %done, label %each");
                body.block("each");
                let current = body.load("ptr", "%value");
                let offset = body.value();
                body.line(&format!("{offset} = mul {iw} %index, {stride}"));
                let item = body.value();
                body.line(&format!(
                    "{item} = getelementptr inbounds i8, ptr {current}, {iw} {offset}"
                ));
                body.line(&format!("call void {}(ptr {item})", quote(&adopter)));
                body.line(&format!("%following = add {iw} %index, 1"));
                body.line("br label %loop");
                body.block("done");
                body.line("ret void");
            }
            None => body.line("ret void"),
        }
        Ok(())
    }
}

impl Ir<'_> {
    /// Makes the buffer of the `String` or `[T]` at `%value` the frontend's
    /// alone, with its elements. A buffer of its own that nothing else holds
    /// is kept, and its elements made unique in turn. One it shares is copied
    /// and its elements copied with it before the shared one is released, so
    /// what the other holders see is untouched. One another allocator made is
    /// taken over as `Mode::Move` takes it.
    fn unique_buffer(&mut self, body: &mut Body, element: Option<&Type>) -> Result<(), String> {
        let word = self.word;
        let iw = self.iw();
        let (stride, align) = match element {
            Some(element) => {
                let (size, align) = self.size_align(element)?;
                (round_up(size, align), align)
            }
            None => (1, 1),
        };
        let data = body.load("ptr", "%value");
        let length_at = body.at("%value", word);
        let length = body.load(&iw, &length_at);
        let capacity_at = body.at("%value", 2 * word);
        let capacity = body.load(&iw, &capacity_at);
        let ops_at = body.at("%value", 3 * word);
        let ops = body.load("ptr", &ops_at);
        let own_ops = body.value();
        body.line(&format!(
            "{own_ops} = call ptr {}()",
            quote(&(self.defined)(BUFFER_OPS))
        ));
        // A buffer with no capacity has no ops and nothing in it.
        let empty = body.value();
        body.line(&format!("{empty} = icmp eq ptr {ops}, null"));
        body.line(&format!("br i1 {empty}, label %done, label %held"));
        body.block("held");
        let free_at = body.at(&ops, 8 + word);
        let free = body.load("ptr", &free_at);
        let release =
            format!("call void {free}(ptr {data}, {iw} {capacity}, {iw} {stride}, {iw} {align})");
        let own = body.value();
        body.line(&format!("{own} = icmp eq ptr {ops}, {own_ops}"));
        body.line(&format!("br i1 {own}, label %own, label %taken"));
        body.block("own");
        let unique = body.value();
        body.line(&format!(
            "{unique} = call i1 {}(ptr {data})",
            quote(&(self.defined)(BUFFER_UNIQUE))
        ));
        body.line(&format!("br i1 {unique}, label %kept, label %shared"));
        // Shared: the elements are copied out of it while it is still held.
        body.block("shared");
        self.fresh_buffer(body, &data, &length, stride, &own_ops);
        self.element_loop(body, element, Mode::Copy, "copied", &length, stride)?;
        body.line(&release);
        body.line("br label %done");
        // Another allocator's: taken over, its elements with it.
        body.block("taken");
        self.fresh_buffer(body, &data, &length, stride, &own_ops);
        body.line(&release);
        self.element_loop(body, element, Mode::Move, "adopted", &length, stride)?;
        body.line("br label %done");
        body.block("kept");
        self.element_loop(body, element, Mode::Unique, "kept", &length, stride)?;
        body.line("br label %done");
        body.block("done");
        body.line("ret void");
        Ok(())
    }

    /// Replaces the buffer at `%value` with a copy of its `length` elements of
    /// `stride` bytes in one of the frontend's own.
    fn fresh_buffer(
        &mut self,
        body: &mut Body,
        data: &str,
        length: &str,
        stride: u64,
        own_ops: &str,
    ) {
        let word = self.word;
        let iw = self.iw();
        let bytes = body.value();
        body.line(&format!("{bytes} = mul {iw} {length}, {stride}"));
        let bytes64 = if word == 8 {
            bytes.clone()
        } else {
            let wide = body.value();
            body.line(&format!("{wide} = zext {iw} {bytes} to i64"));
            wide
        };
        let fresh = body.value();
        body.line(&format!(
            "{fresh} = call ptr {}(i64 {bytes64})",
            quote(&(self.defined)(BUFFER_ALLOCATE))
        ));
        body.line(&format!(
            "call void @llvm.memcpy.p0.p0.{iw}(ptr {fresh}, ptr {data}, {iw} {bytes}, i1 false)"
        ));
        self.declare(
            "llvm.memcpy",
            &format!("declare void @llvm.memcpy.p0.p0.{iw}(ptr, ptr, {iw}, i1)"),
        );
        body.store("ptr", &fresh, "%value");
        let capacity_at = body.at("%value", 2 * word);
        body.store(&iw, length, &capacity_at);
        let ops_at = body.at("%value", 3 * word);
        body.store("ptr", own_ops, &ops_at);
    }

    /// Runs `mode`'s adopter over each of the `length` elements of the buffer
    /// at `%value`, in blocks named after `label`; nothing for elements that
    /// hold no buffer.
    fn element_loop(
        &mut self,
        body: &mut Body,
        element: Option<&Type>,
        mode: Mode,
        label: &str,
        length: &str,
        stride: u64,
    ) -> Result<(), String> {
        let Some(element) = element.filter(|element| self.reaches(element, mode)) else {
            return Ok(());
        };
        let iw = self.iw();
        let adopter = self.adopter(element, mode)?;
        body.line(&format!("br label %{label}.entry"));
        body.block(&format!("{label}.entry"));
        body.line(&format!("br label %{label}.loop"));
        body.block(&format!("{label}.loop"));
        body.line(&format!(
            "%{label}.index = phi {iw} [ 0, %{label}.entry ], [ %{label}.following, %{label}.each ]"
        ));
        body.line(&format!(
            "%{label}.finished = icmp eq {iw} %{label}.index, {length}"
        ));
        body.line(&format!(
            "br i1 %{label}.finished, label %{label}.done, label %{label}.each"
        ));
        body.block(&format!("{label}.each"));
        let current = body.load("ptr", "%value");
        let offset = body.value();
        body.line(&format!("{offset} = mul {iw} %{label}.index, {stride}"));
        let item = body.value();
        body.line(&format!(
            "{item} = getelementptr inbounds i8, ptr {current}, {iw} {offset}"
        ));
        body.line(&format!("call void {}(ptr {item})", quote(&adopter)));
        body.line(&format!("%{label}.following = add {iw} %{label}.index, 1"));
        body.line(&format!("br label %{label}.loop"));
        body.block(&format!("{label}.done"));
        Ok(())
    }
}

/// How a frontend function takes and returns a RIDL signature: its LLVM
/// parameter types, its result type, and whether a leading address receives
/// the result.
pub struct CView {
    pub parameters: Vec<String>,
    pub result: String,
    pub result_address: bool,
}

impl CView {
    /// The C view of `params` and `result` on a target with `word`-byte
    /// pointers.
    #[must_use]
    pub fn of(params: &[(Convention, Type)], result: Option<&Type>, word: u64) -> Self {
        let mut parameters = Vec::new();
        let result = result.map(passed);
        let (result, result_address) = match result.as_ref() {
            None | Some(Type::Scalar(Scalar::Void)) => ("void".to_owned(), false),
            Some(Type::Scalar(scalar)) => (scalar_type(*scalar, word), false),
            Some(_) => ("void".to_owned(), true),
        };
        if result_address {
            parameters.push("ptr".to_owned());
        }
        for (convention, ty) in params {
            parameters.push(match passed(ty) {
                Type::Scalar(scalar) if *convention != Convention::BorrowMut => {
                    scalar_type(scalar, word)
                }
                _ => "ptr".to_owned(),
            });
        }
        Self {
            parameters,
            result,
            result_address,
        }
    }
}

/// The parameters of `function` as `(convention, type)`, the receiver first.
#[must_use]
pub fn parameters(owner: Option<&Type>, function: &FunctionDecl) -> Vec<(Convention, Type)> {
    let mut params = Vec::new();
    if let (Some(receiver), Some(owner)) = (&function.self_param, owner) {
        let convention = match receiver {
            SelfParam::Owned => Convention::Owned,
            SelfParam::Borrow => Convention::Borrow,
            SelfParam::BorrowMut => Convention::BorrowMut,
        };
        params.push((convention, owner.clone()));
    }
    params.extend(
        function
            .params
            .iter()
            .map(|param| (param.convention, passed(&param.ty))),
    );
    params
}

/// `ty` as a call passes it: a sized number (section 27.5) is its storage
/// integer, one integer leaf passed by value (section 22.3), as the frontend
/// passes it; every other type is itself.
#[must_use]
pub fn passed(ty: &Type) -> Type {
    if let Type::Nominal { name, args } = ty
        && args.is_empty()
        && let Some(storage) = crate::types::sized_number_storage(name)
    {
        return Type::Scalar(storage);
    }
    ty.clone()
}

/// The path of an export: `name`, or `Type.name` for a method, the type
/// spelled as the `.ridl` text spells it: `Any Trait.name` for a method of
/// the trait object, which `extend Any Trait` declares. A method of a named
/// instance (section 23.5) is under the instance, written without spaces:
/// `Holder<Number>.read`.
#[must_use]
pub fn path(owner: Option<&Type>, function: &FunctionDecl) -> String {
    match owner {
        Some(ty @ Type::Nominal { args, .. }) if is_instance(args) => {
            let spelled: String = crate::emit::type_text(ty)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            format!("{spelled}.{}", function.name)
        }
        Some(Type::Nominal { name, .. }) => format!("{name}.{}", function.name),
        Some(Type::TraitObject(name)) => format!("Any {name}.{}", function.name),
        _ => function.name.clone(),
    }
}

/// The RIDL symbol of an export (section 24.2).
#[must_use]
pub fn symbol(module: &str, owner: Option<&Type>, function: &FunctionDecl) -> String {
    match owner {
        // `_RIDL1 <module> <type> <method> I <arguments> E`: the method of a
        // named instance, kind `I` over the type's path and the method.
        Some(Type::Nominal { name, args }) if is_instance(args) => {
            crate::mangle::instance_method_symbol(module, name, &function.name, args, None)
        }
        Some(Type::Nominal { name, .. } | Type::TraitObject(name)) => {
            crate::mangle::method_symbol(module, name, &function.name, None)
        }
        _ => crate::mangle::function_symbol(module, &function.name, None),
    }
}

/// Whether `args` make a named instance of a generic declaration: there are
/// some, and none of them is, or holds, a type parameter.
#[must_use]
pub fn is_instance(args: &[Type]) -> bool {
    !args.is_empty() && !args.iter().any(mentions_generic)
}

fn mentions_generic(ty: &Type) -> bool {
    match ty {
        Type::Generic(_) => true,
        Type::Nominal { args, .. } => args.iter().any(mentions_generic),
        Type::Array(inner) | Type::Optional(inner) => mentions_generic(inner),
        Type::Result(ok, err) => mentions_generic(ok) || mentions_generic(err),
        Type::Tuple(items) => items.iter().any(mentions_generic),
        Type::Function { params, result } | Type::BorrowFunction { params, result } => {
            params.iter().any(mentions_generic) || mentions_generic(result)
        }
        Type::Scalar(_) | Type::String | Type::TraitObject(_) => false,
    }
}

/// `T` and `E` of a result: a `Result<T, E>` result makes a failing function
/// (section 22.5). An empty tuple or `Void` `T` is no value.
#[must_use]
pub fn split_result(result: Option<&Type>) -> (Option<Type>, Option<Type>) {
    let (ok, failure) = match result {
        Some(Type::Result(ok, err)) => (Some(passed(ok)), Some((**err).clone())),
        Some(Type::Scalar(Scalar::Void)) | None => (None, None),
        Some(other) => (Some(passed(other)), None),
    };
    (
        ok.filter(|ty| *ty != Type::Tuple(Vec::new()) && *ty != Type::Scalar(Scalar::Void)),
        failure,
    )
}
