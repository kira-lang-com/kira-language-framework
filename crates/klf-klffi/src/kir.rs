//! Mapping a `.klffi` module into KIR (sections 26.2 and 26.3). Each
//! `@FFI.Extern` function becomes a `foreign` KIR function named by its C
//! symbol, which a `call.foreign @symbol(...)` resolves its signature
//! against, and each `@FFI.Struct` and `@FFI.Union` a C-layout type whose
//! offsets, size and alignment the target's C rules give.

use crate::c_record::{CRecord, LayoutError, Records};
use crate::model::{AnnotationKind, FfiItem, FfiModule, FfiType};
use klf_core::{FileId, Name};
use klf_kir::{
    CPrototype, CType, CopyPolicy, Engine, Field, Function, LayoutRule, Linkage, Parameter, Policy,
    Type, TypeDeclaration, TypeKind,
};
use klf_source::Span;
use klf_target::CLayout;

fn span() -> Span {
    Span::empty(FileId::from_u32(0), 0)
}

/// Map an FFI type name to a KIR type. RIDL scalars map to their width; `Void`
/// is unit; `CString`, `RawPtr` and every named FFI type (pointers, opaque
/// structs, callbacks) are a raw pointer.
#[must_use]
pub fn ffi_type_to_kir(ty: &FfiType) -> Type {
    match ty.as_str() {
        "Void" => Type::Unit,
        "Bool" | "I8" | "U8" => Type::Integer(8),
        "I16" | "U16" => Type::Integer(16),
        "I32" | "U32" => Type::Integer(32),
        "I64" | "U64" | "ISize" | "USize" => Type::Integer(64),
        "F32" => Type::Float(32),
        "F64" => Type::Float(64),
        _ => Type::Pointer,
    }
}

/// The layout of the `@FFI.Struct` or `@FFI.Union` named `name` under the
/// C rules `c`.
///
/// # Errors
///
/// Returns why the declaration cannot be laid out: it is not a record, it
/// names an unknown type, it holds itself by value, or a bit-field is not of
/// an integer type or is wider than its type.
pub fn record_layout(module: &FfiModule, name: &str, c: &CLayout) -> Result<CRecord, LayoutError> {
    Records::new(module, c)
        .record(name)
        .map_err(|message| LayoutError {
            declaration: name.to_owned(),
            message,
        })
}

/// A C-layout KIR type for every `@FFI.Struct` and `@FFI.Union` in
/// `module`, laid out by the C rules `c`. A struct is a `struct c` type when
/// the kernel's own C rule places its fields where `c` does, which is what
/// the C shims of section 26.3 spell; otherwise, and for a union, it is an
/// `explicit c` type carrying the offsets, size and alignment. A KIR field
/// has no bit width, so a bit-field is left out of its type's fields; the
/// bits it takes still count in the type's size and in the offsets of the
/// fields after it.
///
/// # Errors
///
/// Returns the first declaration that cannot be laid out, as
/// [`record_layout`] does.
pub fn type_declarations(
    module: &FfiModule,
    c: &CLayout,
) -> Result<Vec<TypeDeclaration>, LayoutError> {
    let mut records = Records::new(module, c);
    let mut declarations = Vec::new();
    let mut unions = Vec::new();
    let mut bit_fields = Vec::new();
    for item in &module.items {
        let FfiItem::Struct(declaration) = item else {
            continue;
        };
        let is_record = declaration
            .annotations
            .iter()
            .any(|a| matches!(a.kind, AnnotationKind::Struct | AnnotationKind::Union));
        if !is_record {
            continue;
        }
        let failed = |message| LayoutError {
            declaration: declaration.name.clone(),
            message,
        };
        let record = records.record(&declaration.name).map_err(failed)?;
        if declaration
            .annotations
            .iter()
            .any(|a| a.kind == AnnotationKind::Union)
        {
            unions.push(declarations.len());
        }
        if record.fields.iter().any(|field| field.bit_offset.is_some()) {
            bit_fields.push(declarations.len());
        }
        let mut fields = Vec::new();
        for (field, placement) in declaration.fields.iter().zip(&record.fields) {
            if placement.bit_offset.is_some() {
                continue;
            }
            let ty = records.shape(&field.ty).map_err(failed)?.kir;
            fields.push(Field {
                offset: Some(placement.offset),
                ..Field::new(Name::new(&field.name), ty)
            });
        }
        let policy = Policy {
            copy: CopyPolicy::Trivial,
            ..Policy::trivial()
        };
        declarations.push(TypeDeclaration {
            kind: TypeKind::Explicit,
            fields,
            size: Some(record.size),
            align: u32::try_from(record.align).ok(),
            ..TypeDeclaration::empty(Name::new(&declaration.name), LayoutRule::C, policy, span())
        });
    }
    while let Some(index) = (0..declarations.len()).find(|&index| {
        declarations[index].kind == TypeKind::Explicit
            && !unions.contains(&index)
            && !bit_fields.contains(&index)
            && kernel_places(&declarations[index], &declarations)
    }) {
        let declaration = &mut declarations[index];
        declaration.kind = TypeKind::Struct;
        declaration.size = None;
        declaration.align = None;
        for field in &mut declaration.fields {
            field.offset = None;
        }
    }
    Ok(declarations)
}

/// Whether the kernel's C rule, given `declarations`, lays the explicit
/// `declaration` out at the same offsets, size and alignment.
fn kernel_places(declaration: &TypeDeclaration, declarations: &[TypeDeclaration]) -> bool {
    let mut plain = declaration.clone();
    plain.kind = TypeKind::Struct;
    plain.size = None;
    plain.align = None;
    for field in &mut plain.fields {
        field.offset = None;
    }
    let mut trial: Vec<TypeDeclaration> = declarations
        .iter()
        .filter(|other| other.name != declaration.name)
        .cloned()
        .collect();
    trial.push(plain);
    let ty = Type::Nominal(declaration.name.clone(), Vec::new());
    let Ok(layout) = klf_layout::try_layout_of(&ty, &trial) else {
        return false;
    };
    let offsets = klf_layout::field_offsets(declaration.name.as_str(), &trial);
    let wanted: Vec<u64> = declaration
        .fields
        .iter()
        .map(|field| field.offset.unwrap_or(0))
        .collect();
    Some(layout.size) == declaration.size
        && u32::try_from(layout.align).ok() == declaration.align
        && offsets == wanted
}

/// The KIR type of `ty` in a foreign signature: `ISize`, `USize` and an
/// `@FFI.Alias` of a C scalar are the integer their width under `c` gives,
/// and every other type is what [`ffi_type_to_kir`] maps it to.
fn signature_type(records: &mut Records<'_>, ty: &FfiType) -> Type {
    let mapped = ffi_type_to_kir(ty);
    if mapped != Type::Pointer && !matches!(ty.as_str(), "ISize" | "USize") {
        return mapped;
    }
    match records.shape(ty) {
        Ok(shape) if matches!(shape.kir, Type::Integer(_) | Type::Float(_)) => shape.kir,
        _ => mapped,
    }
}

/// Build the KIR foreign declarations for every `@FFI.Extern` function in
/// `module`, with each `@FFI.Alias` resolved through the C rules `c`. The
/// declaration's name is the C symbol the linker resolves. A function a
/// record crosses by value carries its C prototype, so the call goes through
/// a C shim; its KIR signature passes the record's address instead.
#[must_use]
pub fn foreign_declarations(module: &FfiModule, c: &CLayout) -> Vec<Function> {
    let mut records = Records::new(module, c);
    let mut functions = Vec::new();
    for item in &module.items {
        let crate::model::FfiItem::Function(function) = item else {
            continue;
        };
        let Some(annotation) = function.annotation(AnnotationKind::Extern) else {
            continue;
        };
        let symbol = annotation.value("symbol").unwrap_or(function.name.as_str());
        let parameter = |name: &str, ty: Type| Parameter {
            name: Name::new(name),
            convention: None,
            context: false,
            ty,
            span: span(),
        };
        let mut parameters: Vec<Parameter> = function
            .parameters
            .iter()
            .map(|(name, ty)| parameter(name, signature_type(&mut records, ty)))
            .collect();
        let mut result = signature_type(&mut records, &function.result);
        let c_prototype = by_value(&mut records, function);
        if matches!(&c_prototype, Some(prototype) if matches!(prototype.result, CType::Value(_))) {
            parameters.insert(0, parameter("result", Type::Pointer));
            result = Type::Unit;
        }
        functions.push(Function {
            name: Name::new(symbol),
            linkage: Linkage::Private,
            abi: klf_kir::FunctionAbi::Platform,
            nounwind: false,
            foreign: true,
            c_prototype,
            generics: Vec::new(),
            attributes: Vec::new(),
            parameters,
            result,
            failure: None,
            engine: Engine::Native,
            blocks: Vec::new(),
            span: span(),
        });
    }
    functions
}

/// The C prototype of `function` when a record crosses it by value, which
/// only a C shim can pass as the target's ABI says (section 26.3). The KIR
/// signature then carries each such record as its address, and a record
/// result is written through an address passed first.
fn by_value(records: &mut Records<'_>, function: &crate::model::FfiFunction) -> Option<CPrototype> {
    let mut parameters = Vec::with_capacity(function.parameters.len());
    for (_, ty) in &function.parameters {
        parameters.push(records.c_type(ty).ok()?);
    }
    let result = records.c_type(&function.result).ok()?;
    let crosses = parameters
        .iter()
        .chain([&result])
        .any(|ty| matches!(ty, CType::Value(_)));
    crosses.then_some(CPrototype {
        parameters,
        result,
        variadic: None,
        syscall: None,
    })
}
