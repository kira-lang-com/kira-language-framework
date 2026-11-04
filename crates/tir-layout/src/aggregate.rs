use tir_types::Type;

use crate::model::{DefinitionKind, FieldLayout, Layout, LayoutError, LayoutField, LayoutRule};

pub(crate) fn layout_fields(
    fields: &[LayoutField],
    kind: DefinitionKind,
    rule: LayoutRule,
    explicit_size: Option<u64>,
    explicit_align: Option<u64>,
    layout_of: &mut impl FnMut(&Type) -> Result<Layout, LayoutError>,
) -> Result<(Layout, Vec<FieldLayout>), LayoutError> {
    if rule != LayoutRule::Explicit
        && fields
            .iter()
            .any(|field| field.explicit_offset.is_some() || field.explicit_bit_offset.is_some())
    {
        return Err(LayoutError::InvalidExplicitLayout(
            "field offsets require an explicit layout".to_owned(),
        ));
    }
    match rule {
        LayoutRule::Ridl => natural_layout(fields, kind, None, layout_of),
        LayoutRule::Packed(alignment) => {
            if !matches!(alignment, 1 | 2 | 4 | 8) {
                return Err(LayoutError::InvalidPacking(alignment));
            }
            natural_layout(fields, kind, Some(u64::from(alignment)), layout_of)
        }
        LayoutRule::C => c_layout(fields, kind, layout_of),
        LayoutRule::Explicit => explicit_layout(
            fields,
            kind,
            explicit_size.ok_or(LayoutError::MissingExplicitSize)?,
            explicit_align.ok_or(LayoutError::MissingExplicitAlignment)?,
            layout_of,
        ),
    }
}

fn natural_layout(
    fields: &[LayoutField],
    kind: DefinitionKind,
    packed: Option<u64>,
    layout_of: &mut impl FnMut(&Type) -> Result<Layout, LayoutError>,
) -> Result<(Layout, Vec<FieldLayout>), LayoutError> {
    let mut extent = 0u64;
    let mut alignment = 1u64;
    let mut field_layouts = Vec::with_capacity(fields.len());
    let mut max_union_size = 0u64;
    for (index, field) in fields.iter().enumerate() {
        if field.bit_width.is_some() || field.flexible {
            return Err(LayoutError::InvalidBitField(field_name(field, index)));
        }
        let layout = layout_of(&field.ty)?;
        let field_alignment = packed.map_or(layout.align, |cap| layout.align.min(cap));
        alignment = alignment.max(field_alignment.max(1));
        let byte_offset = if kind == DefinitionKind::Union {
            max_union_size = max_union_size.max(layout.size);
            0
        } else {
            let byte_offset = align_up(extent, field_alignment.max(1))?;
            extent = byte_offset
                .checked_add(layout.size)
                .ok_or(LayoutError::Overflow)?;
            byte_offset
        };
        field_layouts.push(FieldLayout {
            byte_offset,
            bit_offset: None,
            bit_width: None,
        });
    }
    if kind == DefinitionKind::Union {
        extent = max_union_size;
    }
    Ok((
        Layout::new(align_up(extent, alignment)?, alignment),
        field_layouts,
    ))
}

fn c_layout(
    fields: &[LayoutField],
    kind: DefinitionKind,
    layout_of: &mut impl FnMut(&Type) -> Result<Layout, LayoutError>,
) -> Result<(Layout, Vec<FieldLayout>), LayoutError> {
    if kind == DefinitionKind::Union {
        return c_union_layout(fields, layout_of);
    }
    let mut bit_cursor = 0u64;
    let mut alignment = 1u64;
    let mut field_layouts = Vec::with_capacity(fields.len());
    let mut previous_was_bit_field = false;
    for (index, field) in fields.iter().enumerate() {
        let layout = layout_of(&field.ty)?;
        if field.flexible {
            if index + 1 != fields.len() || !matches!(&field.ty, Type::Array { count: 0, .. }) {
                return Err(LayoutError::InvalidFlexibleArray);
            }
            alignment = alignment.max(layout.align);
            let byte_offset = align_up(bits_to_bytes(bit_cursor)?, layout.align.max(1))?;
            field_layouts.push(FieldLayout {
                byte_offset,
                bit_offset: None,
                bit_width: None,
            });
            bit_cursor = byte_offset.checked_mul(8).ok_or(LayoutError::Overflow)?;
            previous_was_bit_field = false;
            continue;
        }

        if let Some(width) = field.bit_width {
            if field.explicit_bit_offset.is_some() || field.explicit_offset.is_some() {
                return Err(LayoutError::InvalidBitField(field_name(field, index)));
            }
            if !matches!(field.ty, Type::Integer(_)) {
                return Err(LayoutError::InvalidBitField(field_name(field, index)));
            }
            let unit_bits = layout.size.checked_mul(8).ok_or(LayoutError::Overflow)?;
            if u64::from(width) > unit_bits {
                return Err(LayoutError::InvalidBitField(field_name(field, index)));
            }
            if field.name.is_some() {
                alignment = alignment.max(layout.align);
            }
            if width == 0 {
                bit_cursor = align_up(bit_cursor, unit_bits.max(1))?;
                field_layouts.push(FieldLayout {
                    byte_offset: bits_to_bytes(bit_cursor)?,
                    bit_offset: Some(0),
                    bit_width: Some(0),
                });
                previous_was_bit_field = true;
                continue;
            }

            let mut unit_start = if previous_was_bit_field {
                bit_cursor / unit_bits * unit_bits
            } else {
                align_up(bits_to_bytes(bit_cursor)?, layout.align.max(1))?
                    .checked_mul(8)
                    .ok_or(LayoutError::Overflow)?
            };
            let unit_byte = unit_start / 8;
            if unit_byte % layout.align.max(1) != 0 {
                unit_start = align_up(unit_byte, layout.align.max(1))?
                    .checked_mul(8)
                    .ok_or(LayoutError::Overflow)?;
            }
            let mut bit_offset = bit_cursor.saturating_sub(unit_start);
            if bit_offset
                .checked_add(u64::from(width))
                .ok_or(LayoutError::Overflow)?
                > unit_bits
            {
                unit_start = align_up(bit_cursor, unit_bits)?;
                bit_offset = 0;
            }
            let start = unit_start
                .checked_add(bit_offset)
                .ok_or(LayoutError::Overflow)?;
            field_layouts.push(FieldLayout {
                byte_offset: unit_start / 8,
                bit_offset: Some(u16::try_from(bit_offset).map_err(|_| LayoutError::Overflow)?),
                bit_width: Some(width),
            });
            bit_cursor = start
                .checked_add(u64::from(width))
                .ok_or(LayoutError::Overflow)?;
            previous_was_bit_field = true;
        } else {
            if field.explicit_bit_offset.is_some() {
                return Err(LayoutError::InvalidBitField(field_name(field, index)));
            }
            alignment = alignment.max(layout.align);
            let byte_offset = align_up(bits_to_bytes(bit_cursor)?, layout.align.max(1))?;
            field_layouts.push(FieldLayout {
                byte_offset,
                bit_offset: None,
                bit_width: None,
            });
            bit_cursor = byte_offset
                .checked_add(layout.size)
                .and_then(|end| end.checked_mul(8))
                .ok_or(LayoutError::Overflow)?;
            previous_was_bit_field = false;
        }
    }
    let size = align_up(bits_to_bytes(bit_cursor)?, alignment)?;
    Ok((Layout::new(size, alignment), field_layouts))
}

fn c_union_layout(
    fields: &[LayoutField],
    layout_of: &mut impl FnMut(&Type) -> Result<Layout, LayoutError>,
) -> Result<(Layout, Vec<FieldLayout>), LayoutError> {
    let mut extent = 0u64;
    let mut alignment = 1u64;
    let mut field_layouts = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        if field.flexible {
            return Err(LayoutError::InvalidFlexibleArray);
        }
        let layout = layout_of(&field.ty)?;
        if let Some(width) = field.bit_width {
            let storage_bits = layout.size.checked_mul(8).ok_or(LayoutError::Overflow)?;
            if !matches!(field.ty, Type::Integer(_)) || u64::from(width) > storage_bits {
                return Err(LayoutError::InvalidBitField(field_name(field, index)));
            }
            if field.name.is_some() {
                alignment = alignment.max(layout.align);
            }
            extent = extent.max(if width == 0 { 0 } else { layout.size });
            field_layouts.push(FieldLayout {
                byte_offset: 0,
                bit_offset: Some(0),
                bit_width: Some(width),
            });
        } else {
            alignment = alignment.max(layout.align);
            extent = extent.max(layout.size);
            field_layouts.push(FieldLayout {
                byte_offset: 0,
                bit_offset: None,
                bit_width: None,
            });
        }
    }
    Ok((
        Layout::new(align_up(extent, alignment)?, alignment),
        field_layouts,
    ))
}

fn explicit_layout(
    fields: &[LayoutField],
    kind: DefinitionKind,
    size: u64,
    alignment: u64,
    layout_of: &mut impl FnMut(&Type) -> Result<Layout, LayoutError>,
) -> Result<(Layout, Vec<FieldLayout>), LayoutError> {
    if alignment == 0 {
        return Err(LayoutError::InvalidExplicitLayout(
            "explicit alignment must be nonzero".to_owned(),
        ));
    }
    let mut field_layouts = Vec::with_capacity(fields.len());
    let mut occupied = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        if field.flexible {
            return Err(LayoutError::InvalidFlexibleArray);
        }
        let name = field_name(field, index);
        let offset = field
            .explicit_offset
            .ok_or_else(|| LayoutError::MissingExplicitOffset(name.clone()))?;
        let layout = layout_of(&field.ty)?;
        if alignment < layout.align {
            return Err(LayoutError::InvalidExplicitLayout(format!(
                "explicit alignment is too small for field {name}"
            )));
        }
        if offset % layout.align.max(1) != 0 {
            return Err(LayoutError::InvalidExplicitLayout(format!(
                "explicit field {name} is not aligned"
            )));
        }
        let bit_width = field.bit_width;
        let bit_offset = field.explicit_bit_offset;
        if bit_width.is_some() != bit_offset.is_some() {
            return Err(LayoutError::InvalidExplicitLayout(format!(
                "explicit field {name} must provide both bit offset and width"
            )));
        }
        let (start, end) = if let (Some(bit_offset), Some(bit_width)) = (bit_offset, bit_width) {
            let storage_bits = layout.size.checked_mul(8).ok_or(LayoutError::Overflow)?;
            let field_end = u64::from(bit_offset)
                .checked_add(u64::from(bit_width))
                .ok_or(LayoutError::Overflow)?;
            if !matches!(field.ty, Type::Integer(_)) || field_end > storage_bits {
                return Err(LayoutError::InvalidBitField(name));
            }
            let start = offset
                .checked_mul(8)
                .and_then(|value| value.checked_add(u64::from(bit_offset)))
                .ok_or(LayoutError::Overflow)?;
            (
                start,
                start
                    .checked_add(u64::from(bit_width))
                    .ok_or(LayoutError::Overflow)?,
            )
        } else {
            let start = offset.checked_mul(8).ok_or(LayoutError::Overflow)?;
            let end = offset
                .checked_add(layout.size)
                .and_then(|value| value.checked_mul(8))
                .ok_or(LayoutError::Overflow)?;
            (start, end)
        };
        if end > size.checked_mul(8).ok_or(LayoutError::Overflow)? {
            return Err(LayoutError::InvalidExplicitLayout(format!(
                "explicit field {name} exceeds the aggregate size"
            )));
        }
        if kind != DefinitionKind::Union && start != end {
            if occupied
                .iter()
                .any(|(old_start, old_end)| start < *old_end && *old_start < end)
            {
                return Err(LayoutError::InvalidExplicitLayout(format!(
                    "explicit field {name} overlaps another field"
                )));
            }
            occupied.push((start, end));
        }
        field_layouts.push(FieldLayout {
            byte_offset: offset,
            bit_offset,
            bit_width,
        });
    }
    Ok((Layout::new(size, alignment), field_layouts))
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

fn bits_to_bytes(bits: u64) -> Result<u64, LayoutError> {
    bits.checked_add(7)
        .map(|value| value / 8)
        .ok_or(LayoutError::Overflow)
}

fn field_name(field: &LayoutField, index: usize) -> String {
    field
        .name
        .clone()
        .unwrap_or_else(|| format!("field {index}"))
}
