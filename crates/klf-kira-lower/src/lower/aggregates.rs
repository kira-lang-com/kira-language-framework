//! Aggregate storage lowering.

use super::Lowerer;
use super::lists::is_aggregate;
use crate::ty::FeType;
use klf_core::Name;
use klf_kir::{BlockTarget, ConvertKind, StoreMode, Type};

impl Lowerer<'_> {
    /// Copies the bits of `value` into `place`, leaving every count as it is.
    pub(crate) fn copy_bits(&mut self, mode: StoreMode, value: Name, ty: &FeType, place: Name) {
        match ty {
            FeType::List(_) => {
                self.store_struct_fields(mode, value, &Name::new("List"), place);
            }
            FeType::Function { .. } => {
                let record = Name::new(crate::ty::CLOSURE_RECORD);
                self.store_struct_fields(mode, value, &record, place);
            }
            FeType::Struct(name) => {
                let name = name.clone();
                self.store_struct_fields(mode, value, &name, place);
            }
            FeType::Array {
                element,
                length: Some(length),
            } => {
                for index in 0..*length {
                    let index_value = self.fb.const_int(Type::Integer(64), i128::from(index));
                    let source = self.fb.elem_addr(value.clone(), index_value.clone());
                    let destination = self.fb.elem_addr(place.clone(), index_value);
                    let element_value = if is_aggregate(element) {
                        source
                    } else {
                        self.fb.load(source)
                    };
                    self.copy_bits(mode, element_value, element, destination);
                }
            }
            FeType::Enum(name) => {
                let payload_free = self
                    .enums
                    .get(name)
                    .is_some_and(|info| info.variants.iter().all(|case| case.payload.is_empty()));
                if payload_free {
                    // A payload-less enum is just its tag; copy the tag through a
                    // reinterpretation of the destination slot as its tag integer.
                    let variants = self.enums.get(name).map_or(0, |info| info.variants.len());
                    let bits = if variants <= 256 {
                        8
                    } else if variants <= 65_536 {
                        16
                    } else {
                        32
                    };
                    let tag = self.fb.enum_tag(value);
                    let tag_place = self.fb.convert(
                        ConvertKind::Bitcast,
                        place,
                        Type::Address(Box::new(Type::Integer(bits))),
                    );
                    self.fb.store(mode, tag, tag_place);
                } else {
                    let name = name.clone();
                    self.store_payload_enum(mode, value, &name, place);
                }
            }
            _ => self.fb.store(mode, value, place),
        }
    }

    /// Copies an enum with payloads variant by variant: dispatch on the source
    /// tag, copy that variant's payload into the destination, then set the
    /// destination tag.
    fn store_payload_enum(&mut self, mode: StoreMode, value: Name, name: &Name, place: Name) {
        let Some(variants) = self.enums.get(name).map(|info| info.variants.clone()) else {
            self.fb.store(mode, value, place);
            return;
        };
        let join = self.fb.new_block(&[]);
        let mut cases = Vec::with_capacity(variants.len());
        let mut blocks = Vec::with_capacity(variants.len());
        for index in 0..variants.len() {
            let block = self.fb.new_block(&[]);
            let variant = u32::try_from(index).unwrap_or(0);
            cases.push((
                variant,
                BlockTarget {
                    block: block.clone(),
                    arguments: Vec::new(),
                },
            ));
            blocks.push(block);
        }
        self.fb.switch_enum(value.clone(), cases, None);
        for (index, (variant, block)) in variants.iter().zip(blocks).enumerate() {
            let index = u32::try_from(index).unwrap_or(0);
            self.fb.switch_to(&block);
            if let Some(payload_ty) = variant.stored(name) {
                let source = self.fb.enum_payload_addr(value.clone(), index);
                let destination = self.fb.enum_payload_addr(place.clone(), index);
                let payload = if is_aggregate(&payload_ty) {
                    source
                } else {
                    self.fb.load(source)
                };
                self.copy_bits(mode, payload, &payload_ty, destination);
            }
            self.fb.enum_inject(place.clone(), index);
            self.fb.branch(join.clone(), Vec::new());
        }
        self.fb.switch_to(&join);
    }

    /// The address of each payload value of a variant, given the address
    /// `enum.payload.addr` produced: the payload itself, or the fields of the
    /// record that holds several.
    pub(crate) fn payload_places(&mut self, payload: &Name, count: usize) -> Vec<Name> {
        if count == 1 {
            return vec![payload.clone()];
        }
        (0..count)
            .map(|index| {
                let field = u32::try_from(index).unwrap_or(0);
                self.fb.field_addr(payload.clone(), field)
            })
            .collect()
    }

    fn store_struct_fields(&mut self, mode: StoreMode, value: Name, name: &Name, place: Name) {
        let Some(fields) = self.structs.get(name).map(|info| info.fields.clone()) else {
            self.fb.store(mode, value, place);
            return;
        };
        for (index, (_, field_ty)) in fields.into_iter().enumerate() {
            let index = u32::try_from(index).unwrap_or(0);
            let source = self.fb.field_addr(value.clone(), index);
            let destination = self.fb.field_addr(place.clone(), index);
            let field_value = if is_aggregate(&field_ty) {
                source
            } else {
                self.fb.load(source)
            };
            self.copy_bits(mode, field_value, &field_ty, destination);
        }
    }
}
