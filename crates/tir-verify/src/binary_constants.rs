use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryConstantTable, BinaryConstantValue, BinaryFunctionType, BinaryLayout,
    BinaryNominalDefinition, BinaryNominalKind, BinaryType, BinaryTypeTable,
};
use tir_layout::{
    DefinitionKind, EnumVariant, LayoutEngine, LayoutField, LayoutRule, NominalDefinition,
};
use tir_types::{
    Abi, AddressSpace, ClosureType, Convention, CooperativeMatrixUse, EntryKind, FunctionType,
    MatrixScope, ParameterType, TextureAccess, TextureDimension, Type,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn canonical_type_id(
    types: &BinaryTypeTable,
    index: u64,
    module_path: &str,
) -> Result<u64, String> {
    let ty = convert_type(types, index, None, 0)?;
    ty.type_id_in(module_path, &HashMap::new())
        .map(|type_id| type_id.as_u64())
        .map_err(|error| error.to_string())
}

pub(crate) fn binary_layout_engine(types: &BinaryTypeTable) -> Result<LayoutEngine, String> {
    let mut layouts = LayoutEngine::new();
    for definition in &types.definitions {
        layouts
            .add_definition(convert_definition(types, definition)?)
            .map_err(|error| error.to_string())?;
    }
    Ok(layouts)
}

pub(crate) fn binary_type_layout(
    types: &BinaryTypeTable,
    type_index: u64,
) -> Result<tir_layout::Layout, String> {
    let mut layouts = binary_layout_engine(types)?;
    let ty = binary_layout_type(types, type_index)?;
    layouts.layout(&ty).map_err(|error| error.to_string())
}

pub(crate) fn binary_layout_type(types: &BinaryTypeTable, type_index: u64) -> Result<Type, String> {
    convert_type(types, type_index, None, 0)
}

pub(super) fn verify_constants(
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut layouts = LayoutEngine::new();
    for definition in &types.definitions {
        match convert_definition(types, definition) {
            Ok(definition) => {
                if let Err(error) = layouts.add_definition(definition) {
                    diagnostics.push(Diagnostic::error(V2, error.to_string()));
                }
            }
            Err(message) => diagnostics.push(Diagnostic::error(V2, message)),
        }
    }
    for (index, constant) in constants.records.iter().enumerate() {
        let BinaryConstantValue::Bytes(bytes) = &constant.value else {
            continue;
        };
        let ty = match convert_type(types, constant.ty, None, 0) {
            Ok(ty) => ty,
            Err(message) => {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!("constant {index} has invalid type: {message}"),
                ));
                continue;
            }
        };
        match layouts.layout(&ty) {
            Ok(layout) if layout.size == bytes.len() as u64 => {}
            Ok(layout) => diagnostics.push(Diagnostic::error(
                V2,
                format!(
                    "constant {index} has {} bytes; type {} has size {}",
                    bytes.len(),
                    ty,
                    layout.size
                ),
            )),
            Err(error) => diagnostics.push(Diagnostic::error(
                V2,
                format!("constant {index} cannot be laid out: {error}"),
            )),
        }
    }
    diagnostics
}

fn convert_definition(
    types: &BinaryTypeTable,
    definition: &BinaryNominalDefinition,
) -> Result<NominalDefinition, String> {
    let declaration = types
        .declarations
        .get(usize::try_from(definition.declaration).map_err(|_| "declaration index overflow")?)
        .ok_or_else(|| "nominal definition declaration is missing".to_owned())?;
    let scope = Some(definition.declaration);
    let fields = definition
        .fields
        .iter()
        .map(|field| {
            Ok(LayoutField {
                name: Some(field.name.clone()),
                ty: convert_type(types, field.ty, scope, 0)?,
                explicit_offset: field.offset,
                explicit_bit_offset: field
                    .bit_offset
                    .map(u16::try_from)
                    .transpose()
                    .map_err(|_| "bit offset exceeds 16 bits")?,
                bit_width: field
                    .bit_width
                    .map(u16::try_from)
                    .transpose()
                    .map_err(|_| "bit width exceeds 16 bits")?,
                flexible: field.annotations.iter().any(|annotation| {
                    annotation.name == "flexible" && annotation.arguments.is_empty()
                }),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let variants = definition
        .variants
        .iter()
        .map(|variant| {
            Ok(EnumVariant {
                fields: variant
                    .payload
                    .iter()
                    .map(|ty| convert_type(types, *ty, scope, 0).map(LayoutField::positional))
                    .collect::<Result<Vec<_>, _>>()?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let (rule, explicit_size, explicit_align) = match definition.layout {
        BinaryLayout::Ridl => (LayoutRule::Ridl, None, None),
        BinaryLayout::C => (LayoutRule::C, None, None),
        BinaryLayout::Packed(alignment) => (
            LayoutRule::Packed(u8::try_from(alignment).map_err(|_| "packing exceeds 8 bits")?),
            None,
            None,
        ),
        BinaryLayout::Explicit { size, align } => (LayoutRule::Explicit, Some(size), Some(align)),
    };
    Ok(NominalDefinition {
        path: declaration.name.clone(),
        parameters: declaration
            .generic_parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect(),
        opaque: false,
        rule,
        kind: match declaration.kind {
            BinaryNominalKind::Struct => DefinitionKind::Struct,
            BinaryNominalKind::Enum => DefinitionKind::Enum,
            BinaryNominalKind::Explicit => DefinitionKind::Union,
        },
        fields,
        variants,
        tag_type: (declaration.kind == BinaryNominalKind::Enum && rule == LayoutRule::C)
            .then_some(Type::Integer(32)),
        explicit_size,
        explicit_align,
    })
}

fn convert_type(
    types: &BinaryTypeTable,
    index: u64,
    scope: Option<u64>,
    depth: usize,
) -> Result<Type, String> {
    if depth >= 256 {
        return Err("type nesting exceeds 255".to_owned());
    }
    let entry = types
        .entries
        .get(usize::try_from(index).map_err(|_| "type index overflows memory")?)
        .ok_or_else(|| "type index is out of range".to_owned())?;
    let child = |index| convert_type(types, index, scope, depth + 1);
    match entry {
        BinaryType::Integer(bits) => Ok(Type::Integer(
            u16::try_from(*bits).map_err(|_| "integer width exceeds 16 bits")?,
        )),
        BinaryType::Float(float) => Ok(Type::Float(*float)),
        BinaryType::Pointer(space) => Ok(Type::Pointer(address_space(*space)?)),
        BinaryType::Address { pointee, space } => Ok(Type::Address {
            pointee: Box::new(child(*pointee)?),
            space: Some(address_space(*space)?),
        }),
        BinaryType::Unit => Ok(Type::Unit),
        BinaryType::Never => Ok(Type::Never),
        BinaryType::FixedVector { lanes, element } => Ok(Type::FixedVector {
            lanes: u32::try_from(*lanes).map_err(|_| "vector lane count exceeds 32 bits")?,
            element: Box::new(child(*element)?),
        }),
        BinaryType::ScalableVector { element } => {
            Ok(Type::ScalableVector(Box::new(child(*element)?)))
        }
        BinaryType::Tuple(elements) => Ok(Type::Tuple(
            elements
                .iter()
                .map(|element| child(*element))
                .collect::<Result<_, _>>()?,
        )),
        BinaryType::Array { count, element } => Ok(Type::Array {
            count: *count,
            element: Box::new(child(*element)?),
        }),
        BinaryType::Nominal {
            declaration,
            arguments,
        } => {
            let declaration = types
                .declarations
                .get(usize::try_from(*declaration).map_err(|_| "declaration index overflow")?)
                .ok_or_else(|| "nominal declaration is missing".to_owned())?;
            Ok(Type::Nominal {
                path: declaration.name.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| child(*argument))
                    .collect::<Result<_, _>>()?,
            })
        }
        BinaryType::Generic { parameter } => {
            let declaration = scope
                .and_then(|scope| usize::try_from(scope).ok())
                .and_then(|scope| types.declarations.get(scope))
                .ok_or_else(|| {
                    "generic parameter has no enclosing nominal declaration".to_owned()
                })?;
            let parameter = usize::try_from(*parameter)
                .ok()
                .and_then(|index| declaration.generic_parameters.get(index))
                .ok_or_else(|| "generic parameter index is out of range".to_owned())?;
            Ok(Type::Generic(parameter.name.clone()))
        }
        BinaryType::Reference { pointee } => Ok(Type::Reference(Box::new(child(*pointee)?))),
        BinaryType::Unique { pointee } => Ok(Type::Unique(Box::new(child(*pointee)?))),
        BinaryType::Weak { pointee } => Ok(Type::Weak(Box::new(child(*pointee)?))),
        BinaryType::Function(signature) => Ok(Type::Function(convert_function(
            types, signature, scope, depth,
        )?)),
        BinaryType::Closure {
            function_type,
            borrowed,
        } => {
            let BinaryType::Function(signature) = types
                .entries
                .get(usize::try_from(*function_type).map_err(|_| "function type index overflow")?)
                .ok_or_else(|| "closure function type is missing".to_owned())?
            else {
                return Err("closure signature is not a function type".to_owned());
            };
            Ok(Type::Closure(ClosureType {
                borrowed: *borrowed,
                signature: convert_function(types, signature, scope, depth + 1)?,
            }))
        }
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => Ok(Type::Coroutine {
            yield_type: Box::new(child(*yield_type)?),
            resume_type: Box::new(child(*resume_type)?),
            result_type: Box::new(child(*result_type)?),
        }),
        BinaryType::Frame { function_index } => {
            Ok(Type::Frame(format!("function{function_index}")))
        }
        BinaryType::Texture {
            dimension,
            component,
            access,
            multisampled,
        } => Ok(Type::Texture {
            dimension: match dimension {
                0 => TextureDimension::D1,
                1 => TextureDimension::D2,
                2 => TextureDimension::D3,
                3 => TextureDimension::Cube,
                4 => TextureDimension::D1Array,
                5 => TextureDimension::D2Array,
                6 => TextureDimension::CubeArray,
                _ => return Err("texture dimension is invalid".to_owned()),
            },
            component: Box::new(child(*component)?),
            access: match access {
                0 => TextureAccess::Sample,
                1 => TextureAccess::Read,
                2 => TextureAccess::Write,
                3 => TextureAccess::ReadWrite,
                _ => return Err("texture access is invalid".to_owned()),
            },
            multisampled: *multisampled,
        }),
        BinaryType::Sampler => Ok(Type::Sampler),
        BinaryType::AccelerationStructure => Ok(Type::AccelerationStructure),
        BinaryType::VisibleFunctionTable { function_type } => {
            let BinaryType::Function(signature) = types
                .entries
                .get(usize::try_from(*function_type).map_err(|_| "function type index overflow")?)
                .ok_or_else(|| "vft function type is missing".to_owned())?
            else {
                return Err("vft signature is not a function type".to_owned());
            };
            Ok(Type::VisibleFunctionTable(Box::new(convert_function(
                types,
                signature,
                scope,
                depth + 1,
            )?)))
        }
        BinaryType::Tensor { rank, element } => Ok(Type::Tensor {
            count: u32::try_from(*rank).map_err(|_| "tensor rank exceeds 32 bits")?,
            element: Box::new(child(*element)?),
        }),
        BinaryType::RayQuery => Ok(Type::RayQuery),
        BinaryType::HitObject => Ok(Type::HitObject),
        BinaryType::CooperativeMatrix {
            scope,
            rows,
            columns,
            element,
            use_kind,
        } => Ok(Type::CooperativeMatrix {
            scope: match scope {
                0 => MatrixScope::Subgroup,
                1 => MatrixScope::Threadgroup,
                _ => return Err("cooperative-matrix scope is invalid".to_owned()),
            },
            rows: u32::try_from(*rows).map_err(|_| "matrix rows exceed 32 bits")?,
            columns: u32::try_from(*columns).map_err(|_| "matrix columns exceed 32 bits")?,
            element: Box::new(child(*element)?),
            use_kind: match use_kind {
                0 => CooperativeMatrixUse::A,
                1 => CooperativeMatrixUse::B,
                2 => CooperativeMatrixUse::Acc,
                _ => return Err("cooperative-matrix use is invalid".to_owned()),
            },
        }),
        BinaryType::Entry {
            kind,
            root,
            immediate,
        } => Ok(Type::Entry {
            kind: entry_kind(*kind)?,
            root: Box::new(child(*root)?),
            immediate: Box::new(child(*immediate)?),
        }),
    }
}

fn convert_function(
    types: &BinaryTypeTable,
    signature: &BinaryFunctionType,
    scope: Option<u64>,
    depth: usize,
) -> Result<FunctionType, String> {
    Ok(FunctionType {
        parameters: signature
            .parameters
            .iter()
            .map(|parameter| {
                Ok(ParameterType {
                    context: parameter.context,
                    convention: match parameter.convention {
                        0 => Some(Convention::Owned),
                        1 => Some(Convention::Borrow),
                        2 => Some(Convention::BorrowMut),
                        3 => None,
                        _ => return Err("function parameter convention is invalid".to_owned()),
                    },
                    ty: convert_type(types, parameter.ty, scope, depth + 1)?,
                })
            })
            .collect::<Result<_, _>>()?,
        result: Box::new(convert_type(types, signature.result, scope, depth + 1)?),
        failure: signature
            .error
            .map(|ty| convert_type(types, ty, scope, depth + 1).map(Box::new))
            .transpose()?,
        asynchronous: signature.asynchronous,
        abi: if signature.c_abi { Abi::C } else { Abi::Ridl },
        foreign: signature.foreign,
    })
}

fn address_space(code: u8) -> Result<AddressSpace, String> {
    match code {
        0 => Ok(AddressSpace::Generic),
        1 => Ok(AddressSpace::Device),
        2 => Ok(AddressSpace::Constant),
        3 => Ok(AddressSpace::Threadgroup),
        4 => Ok(AddressSpace::Thread),
        5 => Ok(AddressSpace::ObjectPayload),
        6 => Ok(AddressSpace::RayPayload),
        7 => Ok(AddressSpace::HitAttribute),
        8 => Ok(AddressSpace::Tile),
        9 => Ok(AddressSpace::NodeRecord),
        _ => Err("address space is invalid".to_owned()),
    }
}

fn entry_kind(code: u8) -> Result<EntryKind, String> {
    match code {
        0 => Ok(EntryKind::Compute),
        1 => Ok(EntryKind::Object),
        2 => Ok(EntryKind::Mesh),
        3 => Ok(EntryKind::Vertex),
        4 => Ok(EntryKind::Fragment),
        5 => Ok(EntryKind::Tile),
        6 => Ok(EntryKind::Raygen),
        7 => Ok(EntryKind::Intersection),
        8 => Ok(EntryKind::Anyhit),
        9 => Ok(EntryKind::Closesthit),
        10 => Ok(EntryKind::Miss),
        11 => Ok(EntryKind::Callable),
        12 => Ok(EntryKind::Node),
        _ => Err("entry kind is invalid".to_owned()),
    }
}
