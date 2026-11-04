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
mod adopter;
mod entries;
mod entries_metadata;
mod witnesses;

pub use adapters::adapters;
pub use entries::{Receiving, entries, entries_requiring_runtime};
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
