use std::collections::{HashMap, HashSet};

use tir_types::{FloatType, Type};

use crate::aggregate::layout_fields;
use crate::model::{
    DefinitionKind, EnumVariant, Layout, LayoutError, LayoutField, LayoutRule, NominalDefinition,
    NominalLayout,
};

const MAX_OBJECT_SIZE: u64 = 1 << 62;

pub struct LayoutEngine {
    definitions: HashMap<String, NominalDefinition>,
    imported_nominals: HashSet<String>,
    cache: HashMap<Type, NominalLayout>,
    active: Vec<String>,
}

impl LayoutEngine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            definitions: HashMap::new(),
            imported_nominals: HashSet::new(),
            cache: HashMap::new(),
            active: Vec::new(),
        }
    }

    pub fn add_definition(&mut self, definition: NominalDefinition) -> Result<(), LayoutError> {
        if definition.path.trim().is_empty() {
            return Err(LayoutError::InvalidType("empty nominal path".to_owned()));
        }
        if self.definitions.contains_key(&definition.path) {
            return Err(LayoutError::DuplicateDefinition(definition.path));
        }
        if let LayoutRule::Packed(alignment) = definition.rule {
            if !matches!(alignment, 1 | 2 | 4 | 8) {
                return Err(LayoutError::InvalidPacking(alignment));
            }
        }
        if definition.opaque
            && (definition.kind != DefinitionKind::Struct
                || !definition.fields.is_empty()
                || !definition.variants.is_empty())
        {
            return Err(LayoutError::InvalidType(
                "opaque nominal types must be fieldless structs".to_owned(),
            ));
        }
        if definition.kind == DefinitionKind::Enum && !definition.fields.is_empty() {
            return Err(LayoutError::InvalidType(
                "enum definitions use variant payloads, not struct fields".to_owned(),
            ));
        }
        if definition.kind == DefinitionKind::Enum && definition.rule == LayoutRule::Explicit {
            return Err(LayoutError::InvalidType(
                "explicit layouts are not supported for enums without per-variant payload offsets"
                    .to_owned(),
            ));
        }
        if definition.kind != DefinitionKind::Enum && !definition.variants.is_empty() {
            return Err(LayoutError::InvalidType(
                "only enum definitions may declare variants".to_owned(),
            ));
        }
        for field in &definition.fields {
            field
                .ty
                .validate()
                .map_err(|error| LayoutError::InvalidType(error.to_string()))?;
        }
        for field in definition
            .variants
            .iter()
            .flat_map(|variant| &variant.fields)
        {
            field
                .ty
                .validate()
                .map_err(|error| LayoutError::InvalidType(error.to_string()))?;
        }
        self.imported_nominals.remove(&definition.path);
        self.definitions.insert(definition.path.clone(), definition);
        self.cache.clear();
        Ok(())
    }

    #[must_use]
    pub fn definition(&self, path: &str) -> Option<&NominalDefinition> {
        self.definitions.get(path)
    }

    pub fn register_imported_nominal(&mut self, path: impl Into<String>) {
        let path = path.into();
        if !self.definitions.contains_key(&path) {
            self.imported_nominals.insert(path);
        }
    }

    #[must_use]
    pub fn is_imported_nominal(&self, path: &str) -> bool {
        self.imported_nominals.contains(path)
    }

    pub fn layout(&mut self, ty: &Type) -> Result<Layout, LayoutError> {
        self.layout_inner(ty)
    }

    pub fn nominal_layout(&mut self, ty: &Type) -> Result<NominalLayout, LayoutError> {
        let Type::Nominal { path, arguments } = ty else {
            return Err(LayoutError::InvalidType(
                "nominal_layout requires a nominal type".to_owned(),
            ));
        };
        ty.validate()
            .map_err(|error| LayoutError::InvalidType(error.to_string()))?;
        let key = Type::Nominal {
            path: path.clone(),
            arguments: arguments.clone(),
        };
        if let Some(layout) = self.cache.get(&key) {
            return Ok(layout.clone());
        }
        if self.active.iter().any(|active| active == path) {
            return Err(LayoutError::RecursiveNominal(path.clone()));
        }
        let definition = self
            .definitions
            .get(path)
            .cloned()
            .ok_or_else(|| LayoutError::UnknownNominal(path.clone()))?;
        if definition.parameters.len() != arguments.len() {
            return Err(LayoutError::GenericArgumentCount {
                path: path.clone(),
                expected: definition.parameters.len(),
                actual: arguments.len(),
            });
        }
        let bindings: HashMap<_, _> = definition
            .parameters
            .iter()
            .cloned()
            .zip(arguments.iter().cloned())
            .collect();
        self.active.push(path.clone());
        let result = if definition.opaque {
            Ok(NominalLayout {
                layout: Layout::new(8, 8),
                fields: Vec::new(),
                variants: Vec::new(),
                payload_offset: None,
            })
        } else if definition.kind == DefinitionKind::Enum {
            self.layout_enum(&definition, &bindings)
        } else {
            let fields: Vec<_> = definition
                .fields
                .iter()
                .map(|field| LayoutField {
                    name: field.name.clone(),
                    ty: field.ty.substitute(&bindings),
                    explicit_offset: field.explicit_offset,
                    explicit_bit_offset: field.explicit_bit_offset,
                    bit_width: field.bit_width,
                    flexible: field.flexible,
                })
                .collect();
            layout_fields(
                &fields,
                definition.kind,
                definition.rule,
                definition.explicit_size,
                definition.explicit_align,
                &mut |field_type| self.layout_inner(field_type),
            )
            .map(|(layout, fields)| NominalLayout {
                layout,
                fields,
                variants: Vec::new(),
                payload_offset: None,
            })
        };
        self.active.pop();
        let result = result?;
        if result.layout.size >= MAX_OBJECT_SIZE {
            return Err(LayoutError::ObjectTooLarge);
        }
        self.cache.insert(key, result.clone());
        Ok(result)
    }

    fn layout_enum(
        &mut self,
        definition: &NominalDefinition,
        bindings: &HashMap<String, Type>,
    ) -> Result<NominalLayout, LayoutError> {
        if definition.rule == LayoutRule::C {
            if !definition
                .variants
                .iter()
                .all(|variant| variant.fields.is_empty())
            {
                return Err(LayoutError::InvalidType(
                    "C enumerators cannot carry payloads".to_owned(),
                ));
            }
            let tag_type = definition
                .tag_type
                .as_ref()
                .ok_or(LayoutError::MissingEnumTag)?;
            if !matches!(tag_type, Type::Integer(_)) {
                return Err(LayoutError::InvalidType(
                    "C enum underlying type must be an integer".to_owned(),
                ));
            }
            return Ok(NominalLayout {
                layout: self.layout_inner(&tag_type.substitute(bindings))?,
                fields: Vec::new(),
                variants: vec![Vec::new(); definition.variants.len()],
                payload_offset: None,
            });
        }
        let inferred_tag = Type::Integer(enum_tag_width(definition.variants.len()));
        let tag_type = match definition.rule {
            LayoutRule::Ridl | LayoutRule::Packed(_) => {
                if definition
                    .tag_type
                    .as_ref()
                    .is_some_and(|tag| tag != &inferred_tag)
                {
                    return Err(LayoutError::InvalidType(
                        "RIDL enum tag width is determined by its variant count".to_owned(),
                    ));
                }
                inferred_tag
            }
            LayoutRule::Explicit => definition
                .tag_type
                .clone()
                .ok_or(LayoutError::MissingEnumTag)?,
            LayoutRule::C => unreachable!(),
        }
        .substitute(bindings);
        let tag = self.layout_inner(&tag_type)?;

        if definition.rule != LayoutRule::Explicit
            && let Some((variant_index, payload_type)) = self.pointer_niche_variant(definition)
        {
            let mut payload = self.layout_inner(&payload_type.substitute(bindings))?;
            if let LayoutRule::Packed(alignment) = definition.rule {
                payload.align = payload.align.min(u64::from(alignment));
            }
            let mut variants = vec![Vec::new(); definition.variants.len()];
            variants[variant_index].push(crate::model::FieldLayout {
                byte_offset: 0,
                bit_offset: None,
                bit_width: None,
            });
            return Ok(NominalLayout {
                layout: payload,
                fields: Vec::new(),
                variants,
                payload_offset: Some(0),
            });
        }

        let mut variant_layouts = Vec::with_capacity(definition.variants.len());
        let mut payload_size = 0u64;
        let mut payload_align = 1u64;
        for variant in &definition.variants {
            let fields = variant_fields(variant, bindings);
            let (layout, field_layouts) = if definition.rule == LayoutRule::Explicit {
                let size = definition
                    .explicit_size
                    .ok_or(LayoutError::MissingExplicitSize)?;
                let align = definition
                    .explicit_align
                    .ok_or(LayoutError::MissingExplicitAlignment)?;
                let (layout, fields) = layout_fields(
                    &fields,
                    DefinitionKind::Struct,
                    LayoutRule::Explicit,
                    Some(size),
                    Some(align),
                    &mut |field_type| self.layout_inner(field_type),
                )?;
                if fields.iter().any(|field| field.byte_offset < tag.size) {
                    return Err(LayoutError::InvalidExplicitLayout(
                        "enum payload overlaps its tag".to_owned(),
                    ));
                }
                (layout, fields)
            } else {
                layout_fields(
                    &fields,
                    DefinitionKind::Struct,
                    definition.rule,
                    None,
                    None,
                    &mut |field_type| self.layout_inner(field_type),
                )?
            };
            payload_size = payload_size.max(layout.size);
            payload_align = payload_align.max(layout.align);
            variant_layouts.push(field_layouts);
        }
        if definition.rule == LayoutRule::Explicit {
            let size = definition
                .explicit_size
                .ok_or(LayoutError::MissingExplicitSize)?;
            let align = definition
                .explicit_align
                .ok_or(LayoutError::MissingExplicitAlignment)?;
            if size < tag.size || align < tag.align {
                return Err(LayoutError::InvalidExplicitLayout(
                    "explicit enum layout is smaller than its tag".to_owned(),
                ));
            }
            return Ok(NominalLayout {
                layout: Layout::new(size, align),
                fields: Vec::new(),
                variants: variant_layouts,
                payload_offset: Some(0),
            });
        }

        let packing = match definition.rule {
            LayoutRule::Packed(alignment) => Some(u64::from(alignment)),
            LayoutRule::Ridl => None,
            LayoutRule::C | LayoutRule::Explicit => unreachable!(),
        };
        let tag_align = packing.map_or(tag.align, |cap| tag.align.min(cap));
        let payload_align = packing.map_or(payload_align, |cap| payload_align.min(cap));
        let payload_offset = align_up(tag.size, payload_align)?;
        let aggregate_align = tag_align.max(payload_align);
        let computed_size = align_up(
            payload_offset
                .checked_add(payload_size)
                .ok_or(LayoutError::Overflow)?,
            aggregate_align,
        )?;
        for variant in &mut variant_layouts {
            for field in variant {
                field.byte_offset = field
                    .byte_offset
                    .checked_add(payload_offset)
                    .ok_or(LayoutError::Overflow)?;
            }
        }
        Ok(NominalLayout {
            layout: Layout::new(computed_size, aggregate_align),
            fields: Vec::new(),
            variants: variant_layouts,
            payload_offset: Some(payload_offset),
        })
    }

    fn pointer_niche_variant(&self, definition: &NominalDefinition) -> Option<(usize, Type)> {
        if definition.variants.len() != 2 {
            return None;
        }
        let empty = definition
            .variants
            .iter()
            .position(|variant| variant.fields.is_empty())?;
        let payload = definition
            .variants
            .iter()
            .enumerate()
            .find(|(index, variant)| *index != empty && variant.fields.len() == 1)?;
        let ty = &payload.1.fields[0].ty;
        match ty {
            Type::Closure(_) => Some((payload.0, ty.clone())),
            Type::Nominal { path, .. }
                if self
                    .definitions
                    .get(path)
                    .is_some_and(|nominal| nominal.opaque) =>
            {
                Some((payload.0, ty.clone()))
            }
            _ => None,
        }
    }

    fn layout_inner(&mut self, ty: &Type) -> Result<Layout, LayoutError> {
        ty.validate()
            .map_err(|error| LayoutError::InvalidType(error.to_string()))?;
        let layout = match ty {
            Type::Integer(bits) => scalar_layout(*bits),
            Type::Float(kind) => scalar_layout(kind.bits()),
            Type::Pointer(_)
            | Type::Reference(_)
            | Type::Unique(_)
            | Type::Weak(_)
            | Type::Function(_)
            | Type::Coroutine { .. }
            | Type::Texture { .. }
            | Type::Sampler
            | Type::AccelerationStructure
            | Type::VisibleFunctionTable(_)
            | Type::Tensor { .. }
            | Type::Entry { .. } => Ok(Layout::new(8, 8)),
            Type::Unit | Type::Never => Ok(Layout::new(0, 1)),
            Type::Code => Err(LayoutError::InvalidType(
                "code values are not TIR memory types".to_owned(),
            )),
            Type::Closure(_) => Ok(Layout::new(16, 8)),
            Type::FixedVector { lanes, element } => self.layout_vector(*lanes, element),
            Type::ScalableVector(_) | Type::Generic(_) | Type::Address { .. } | Type::Frame(_) => {
                Err(LayoutError::AddressOnlyType)
            }
            Type::Tuple(fields) => {
                let fields = fields
                    .iter()
                    .map(|ty| LayoutField::named("", ty.clone()))
                    .collect::<Vec<_>>();
                layout_fields(
                    &fields,
                    DefinitionKind::Struct,
                    LayoutRule::Ridl,
                    None,
                    None,
                    &mut |field_type| self.layout_inner(field_type),
                )
                .map(|(layout, _)| layout)
            }
            Type::Array { count, element } => {
                if let Some(bits) = packed_float_bits(element) {
                    let total_bits = count
                        .checked_mul(u64::from(bits))
                        .ok_or(LayoutError::Overflow)?;
                    return Ok(Layout::new(total_bits.div_ceil(8), 1));
                }
                let element_layout = self.layout_inner(element)?;
                let stride = align_up(element_layout.size, element_layout.align)?;
                let size = stride.checked_mul(*count).ok_or(LayoutError::Overflow)?;
                Ok(Layout::new(size, element_layout.align))
            }
            Type::Nominal { .. } => self.nominal_layout(ty).map(|nominal| nominal.layout),
            Type::RayQuery | Type::HitObject | Type::CooperativeMatrix { .. } => {
                Err(LayoutError::RegisterOnlyType)
            }
        }?;
        if layout.size >= MAX_OBJECT_SIZE {
            return Err(LayoutError::ObjectTooLarge);
        }
        Ok(layout)
    }

    fn layout_vector(&mut self, lanes: u32, element: &Type) -> Result<Layout, LayoutError> {
        if let Some(bits) = packed_float_bits(element) {
            let total_bits = u64::from(lanes)
                .checked_mul(u64::from(bits))
                .ok_or(LayoutError::Overflow)?;
            return Ok(Layout::new(total_bits.div_ceil(8), 1));
        }
        let element_layout = self.layout_inner(element)?;
        let power = u64::from(lanes).next_power_of_two();
        let alignment = element_layout
            .size
            .checked_mul(power)
            .ok_or(LayoutError::Overflow)?
            .min(16)
            .max(1);
        let size = element_layout
            .size
            .checked_mul(u64::from(lanes))
            .ok_or(LayoutError::Overflow)?;
        Ok(Layout::new(align_up(size, alignment)?, alignment))
    }
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn variant_fields(variant: &EnumVariant, bindings: &HashMap<String, Type>) -> Vec<LayoutField> {
    variant
        .fields
        .iter()
        .map(|field| LayoutField {
            name: field.name.clone(),
            ty: field.ty.substitute(bindings),
            explicit_offset: field.explicit_offset,
            explicit_bit_offset: field.explicit_bit_offset,
            bit_width: field.bit_width,
            flexible: field.flexible,
        })
        .collect()
}

fn enum_tag_width(variants: usize) -> u16 {
    match variants {
        0..=256 => 8,
        257..=65_536 => 16,
        _ => 32,
    }
}

fn scalar_layout(bits: u16) -> Result<Layout, LayoutError> {
    let bytes = u64::from(bits.div_ceil(8)).max(1);
    let size = bytes
        .checked_next_power_of_two()
        .ok_or(LayoutError::Overflow)?;
    Ok(Layout::new(size, size.min(16)))
}

fn packed_float_bits(ty: &Type) -> Option<u16> {
    match ty {
        Type::Float(FloatType::F6E2M3 | FloatType::F6E3M2) => Some(6),
        Type::Float(FloatType::F4E2M1) => Some(4),
        _ => None,
    }
}

fn align_up(value: u64, alignment: u64) -> Result<u64, LayoutError> {
    if alignment == 0 {
        return Err(LayoutError::InvalidExplicitLayout(
            "alignment must be nonzero".to_owned(),
        ));
    }
    let remainder = value % alignment;
    if remainder == 0 {
        Ok(value)
    } else {
        value
            .checked_add(alignment - remainder)
            .ok_or(LayoutError::Overflow)
    }
}
