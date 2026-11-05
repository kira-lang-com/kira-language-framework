use super::syntax::{find_punctuation, item_error, path_end, span_at, token_text};
use crate::text::lex;
use crate::types::{
    BinaryFunctionParameter, BinaryFunctionType, BinaryNominalDeclaration, BinaryType,
};
use crate::{TextItem, TextItemKind, TextModule, TextParseError};
use std::collections::HashMap;
use tir_diag::Span;
use tir_types::{
    Abi, AddressSpace, Convention, CooperativeMatrixUse, EntryKind, MatrixScope, TextureAccess,
    TextureDimension, Type,
};

pub(super) struct TypeTableBuilder<'a> {
    declarations: &'a [BinaryNominalDeclaration],
    declaration_names: &'a HashMap<String, u64>,
    function_indices: &'a HashMap<String, u64>,
    entries: Vec<BinaryType>,
    indices: HashMap<BinaryType, u64>,
}

impl TypeTableBuilder<'_> {
    pub(super) fn new<'a>(
        declarations: &'a [BinaryNominalDeclaration],
        declaration_names: &'a HashMap<String, u64>,
        function_indices: &'a HashMap<String, u64>,
    ) -> TypeTableBuilder<'a> {
        TypeTableBuilder {
            declarations,
            declaration_names,
            function_indices,
            entries: Vec::new(),
            indices: HashMap::new(),
        }
    }

    pub(super) fn into_entries(self) -> Vec<BinaryType> {
        self.entries
    }

    pub(super) fn function_indices(&self) -> &HashMap<String, u64> {
        self.function_indices
    }

    pub(super) fn intern_type(
        &mut self,
        ty: &Type,
        scope: u64,
        item: TextItem,
    ) -> Result<u64, TextParseError> {
        let binary = self.lower_type(ty, scope, item, 0)?;
        if let Some(index) = self.indices.get(&binary) {
            return Ok(*index);
        }
        let index = u64::try_from(self.entries.len())
            .map_err(|_| item_error(item, Span::new(0, 0), "TYPES entry count exceeds ULEB128"))?;
        self.entries.push(binary.clone());
        self.indices.insert(binary, index);
        Ok(index)
    }

    pub(super) fn intern_external_type(
        &mut self,
        ty: &Type,
        item: TextItem,
    ) -> Result<u64, TextParseError> {
        self.intern_type(ty, u64::MAX, item)
    }

    fn lower_type(
        &mut self,
        ty: &Type,
        scope: u64,
        item: TextItem,
        depth: usize,
    ) -> Result<BinaryType, TextParseError> {
        if depth >= 256 {
            return Err(item_error(
                item,
                Span::new(0, 0),
                "type nesting exceeds 255",
            ));
        }
        let child = |this: &mut Self, ty: &Type| this.intern_type_depth(ty, scope, item, depth + 1);
        Ok(match ty {
            Type::Integer(bits) => BinaryType::Integer(u64::from(*bits)),
            Type::Float(float) => BinaryType::Float(*float),
            Type::Pointer(space) => BinaryType::Pointer(address_space(*space)),
            Type::Code => {
                return Err(item_error(
                    item,
                    Span::new(0, 0),
                    "the TIR binary type table has no 'code' type tag",
                ));
            }
            Type::Unit => BinaryType::Unit,
            Type::Never => BinaryType::Never,
            Type::FixedVector { lanes, element } => BinaryType::FixedVector {
                lanes: u64::from(*lanes),
                element: child(self, element)?,
            },
            Type::ScalableVector(element) => BinaryType::ScalableVector {
                element: child(self, element)?,
            },
            Type::Tuple(elements) => BinaryType::Tuple(
                elements
                    .iter()
                    .map(|ty| child(self, ty))
                    .collect::<Result<_, _>>()?,
            ),
            Type::Array { count, element } => BinaryType::Array {
                count: *count,
                element: child(self, element)?,
            },
            Type::Nominal { path, arguments } => {
                let declaration = self.declaration_names.get(path).copied().ok_or_else(|| {
                    item_error(
                        item,
                        Span::new(0, 0),
                        "nominal type path has no declaration",
                    )
                })?;
                let expected = self.declarations[declaration as usize]
                    .generic_parameters
                    .len();
                if expected != arguments.len() {
                    return Err(item_error(
                        item,
                        Span::new(0, 0),
                        "nominal type argument count does not match its declaration",
                    ));
                }
                BinaryType::Nominal {
                    declaration,
                    arguments: arguments
                        .iter()
                        .map(|ty| child(self, ty))
                        .collect::<Result<_, _>>()?,
                }
            }
            Type::Generic(name) => {
                let index = usize::try_from(scope)
                    .ok()
                    .and_then(|scope| self.declarations.get(scope))
                    .and_then(|declaration| {
                        declaration
                            .generic_parameters
                            .iter()
                            .position(|parameter| parameter.name == *name)
                    })
                    .ok_or_else(|| {
                        item_error(
                            item,
                            Span::new(0, 0),
                            "generic type is not declared by this nominal type",
                        )
                    })?;
                BinaryType::Generic {
                    parameter: index as u64,
                }
            }
            Type::Reference(pointee) => BinaryType::Reference {
                pointee: child(self, pointee)?,
            },
            Type::Unique(pointee) => BinaryType::Unique {
                pointee: child(self, pointee)?,
            },
            Type::Weak(pointee) => BinaryType::Weak {
                pointee: child(self, pointee)?,
            },
            Type::Address { pointee, space } => BinaryType::Address {
                pointee: child(self, pointee)?,
                space: space.map_or(0, address_space),
            },
            Type::Function(signature) => {
                BinaryType::Function(self.lower_function(signature, scope, item, depth)?)
            }
            Type::Closure(closure) => {
                let function = self.intern_type_depth(
                    &Type::Function(closure.signature.clone()),
                    scope,
                    item,
                    depth + 1,
                )?;
                BinaryType::Closure {
                    function_type: function,
                    borrowed: closure.borrowed,
                }
            }
            Type::Coroutine {
                yield_type,
                resume_type,
                result_type,
            } => BinaryType::Coroutine {
                yield_type: child(self, yield_type)?,
                resume_type: child(self, resume_type)?,
                result_type: child(self, result_type)?,
            },
            Type::Frame(path) => BinaryType::Frame {
                function_index: self.function_indices.get(path).copied().ok_or_else(|| {
                    item_error(
                        item,
                        Span::new(0, 0),
                        "frame type names an undeclared function",
                    )
                })?,
            },
            Type::Texture {
                dimension,
                component,
                access,
                multisampled,
            } => BinaryType::Texture {
                dimension: dimension_code(*dimension),
                component: child(self, component)?,
                access: access_code(*access),
                multisampled: *multisampled,
            },
            Type::Sampler => BinaryType::Sampler,
            Type::AccelerationStructure => BinaryType::AccelerationStructure,
            Type::VisibleFunctionTable(signature) => {
                let function = self.intern_type_depth(
                    &Type::Function((**signature).clone()),
                    scope,
                    item,
                    depth + 1,
                )?;
                BinaryType::VisibleFunctionTable {
                    function_type: function,
                }
            }
            Type::Tensor { count, element } => BinaryType::Tensor {
                rank: u64::from(*count),
                element: child(self, element)?,
            },
            Type::RayQuery => BinaryType::RayQuery,
            Type::HitObject => BinaryType::HitObject,
            Type::CooperativeMatrix {
                scope: matrix_scope,
                rows,
                columns,
                element,
                use_kind,
            } => BinaryType::CooperativeMatrix {
                scope: u8::from(*matrix_scope == MatrixScope::Threadgroup),
                rows: u64::from(*rows),
                columns: u64::from(*columns),
                element: child(self, element)?,
                use_kind: match use_kind {
                    CooperativeMatrixUse::A => 0,
                    CooperativeMatrixUse::B => 1,
                    CooperativeMatrixUse::Acc => 2,
                },
            },
            Type::Entry {
                kind,
                root,
                immediate,
            } => BinaryType::Entry {
                kind: entry_kind(*kind),
                root: child(self, root)?,
                immediate: child(self, immediate)?,
            },
        })
    }

    fn intern_type_depth(
        &mut self,
        ty: &Type,
        scope: u64,
        item: TextItem,
        depth: usize,
    ) -> Result<u64, TextParseError> {
        if depth >= 256 {
            return Err(item_error(
                item,
                Span::new(0, 0),
                "type nesting exceeds 255",
            ));
        }
        let binary = self.lower_type(ty, scope, item, depth)?;
        if let Some(index) = self.indices.get(&binary) {
            return Ok(*index);
        }
        let index = u64::try_from(self.entries.len())
            .map_err(|_| item_error(item, Span::new(0, 0), "TYPES entry count exceeds ULEB128"))?;
        self.entries.push(binary.clone());
        self.indices.insert(binary, index);
        Ok(index)
    }

    fn lower_function(
        &mut self,
        function: &tir_types::FunctionType,
        scope: u64,
        item: TextItem,
        depth: usize,
    ) -> Result<BinaryFunctionType, TextParseError> {
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| {
                Ok(BinaryFunctionParameter {
                    convention: match parameter.convention {
                        Some(Convention::Owned) => 0,
                        Some(Convention::Borrow) => 1,
                        Some(Convention::BorrowMut) => 2,
                        None => 3,
                    },
                    context: parameter.context,
                    ty: self.intern_type_depth(&parameter.ty, scope, item, depth + 1)?,
                })
            })
            .collect::<Result<Vec<_>, TextParseError>>()?;
        Ok(BinaryFunctionType {
            parameters,
            result: self.intern_type_depth(&function.result, scope, item, depth + 1)?,
            error: function
                .failure
                .as_deref()
                .map(|ty| self.intern_type_depth(ty, scope, item, depth + 1))
                .transpose()?,
            asynchronous: function.asynchronous,
            c_abi: function.abi == Abi::C,
            foreign: function.foreign,
        })
    }
}

pub(super) fn function_indices(
    module: &TextModule,
) -> Result<HashMap<String, u64>, TextParseError> {
    let mut result = HashMap::new();
    let mut index = 0_u64;
    for item in module
        .items
        .iter()
        .filter(|item| matches!(item.kind, TextItemKind::Function | TextItemKind::Entry))
    {
        let tokens = lex(module.item_text(*item))
            .map_err(|error| item_error(*item, error.span, error.message))?;
        let at = find_punctuation(&tokens, 0, '@')
            .ok_or_else(|| item_error(*item, Span::new(0, 0), "function has no name"))?;
        let end = path_end(&tokens, at + 1);
        let name = token_text(module.item_text(*item), &tokens, at + 1, end).unwrap_or_default();
        if result.insert(name, index).is_some() {
            return Err(item_error(
                *item,
                span_at(&tokens, at),
                "duplicate function name",
            ));
        }
        index += 1;
    }
    Ok(result)
}

fn address_space(space: AddressSpace) -> u8 {
    match space {
        AddressSpace::Generic => 0,
        AddressSpace::Device => 1,
        AddressSpace::Constant => 2,
        AddressSpace::Threadgroup => 3,
        AddressSpace::Thread => 4,
        AddressSpace::ObjectPayload => 5,
        AddressSpace::RayPayload => 6,
        AddressSpace::HitAttribute => 7,
        AddressSpace::Tile => 8,
        AddressSpace::NodeRecord => 9,
    }
}

fn dimension_code(dimension: TextureDimension) -> u8 {
    match dimension {
        TextureDimension::D1 => 0,
        TextureDimension::D2 => 1,
        TextureDimension::D3 => 2,
        TextureDimension::Cube => 3,
        TextureDimension::D1Array => 4,
        TextureDimension::D2Array => 5,
        TextureDimension::CubeArray => 6,
    }
}

fn access_code(access: TextureAccess) -> u8 {
    match access {
        TextureAccess::Sample => 0,
        TextureAccess::Read => 1,
        TextureAccess::Write => 2,
        TextureAccess::ReadWrite => 3,
    }
}

fn entry_kind(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Compute => 0,
        EntryKind::Object => 1,
        EntryKind::Mesh => 2,
        EntryKind::Vertex => 3,
        EntryKind::Fragment => 4,
        EntryKind::Tile => 5,
        EntryKind::Raygen => 6,
        EntryKind::Intersection => 7,
        EntryKind::Anyhit => 8,
        EntryKind::Closesthit => 9,
        EntryKind::Miss => 10,
        EntryKind::Callable => 11,
        EntryKind::Node => 12,
    }
}
