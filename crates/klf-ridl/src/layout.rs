//! RIDL data layout (section 21).

use std::collections::BTreeMap;

use crate::types::{Decl, EnumDecl, Registry, Scalar, StructDecl, Type};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    pub size: u64,
    pub align: u64,
}

impl Layout {
    #[must_use]
    pub const fn new(size: u64, align: u64) -> Self {
        Self { size, align }
    }

    /// The stride of a type equals its size (section 23.1).
    #[must_use]
    pub const fn stride(self) -> u64 {
        self.size
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutError {
    UnknownType(String),
    Unrepresentable(String),
    ArgCountMismatch {
        name: String,
        expected: usize,
        got: usize,
    },
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::UnknownType(n) => write!(f, "unknown type `{n}`"),
            LayoutError::Unrepresentable(m) => write!(f, "type has no RIDL layout: {m}"),
            LayoutError::ArgCountMismatch {
                name,
                expected,
                got,
            } => write!(
                f,
                "`{name}` takes {expected} generic argument(s), got {got}"
            ),
        }
    }
}

impl std::error::Error for LayoutError {}

/// A struct field placed at a concrete offset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldPlacement {
    pub name: String,
    pub offset: u64,
    pub layout: Layout,
}

/// Where an enum keeps its tag and its payloads (section 21.3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumShape {
    /// Bytes of the tag at offset 0; 0 for a pointer-niche enum, whose
    /// payload-less case is a zero first word.
    pub tag_size: u64,
    pub niche: bool,
    pub payload_offset: u64,
    /// Per case in declaration order, each payload value and its offset from
    /// the start of the enum.
    pub cases: Vec<Vec<(Type, u64)>>,
}

pub struct LayoutEngine<'r> {
    registry: &'r Registry,
    word: u64,
}

const fn round_up(value: u64, align: u64) -> u64 {
    if align == 0 {
        return value;
    }
    value.div_ceil(align) * align
}

impl<'r> LayoutEngine<'r> {
    #[must_use]
    pub const fn new(registry: &'r Registry, pointer_width_bytes: u64) -> Self {
        Self {
            registry,
            word: pointer_width_bytes,
        }
    }

    #[must_use]
    pub const fn word(&self) -> u64 {
        self.word
    }

    /// The declarations nominal types resolve against.
    #[must_use]
    pub const fn registry(&self) -> &'r Registry {
        self.registry
    }

    /// Layout of `ty`, resolving nominal types through the registry.
    ///
    /// # Errors
    /// Returns [`LayoutError`] for an unknown nominal type, a `Void` or generic
    /// type with no concrete layout, or a wrong generic argument count.
    pub fn layout_of(&self, ty: &Type) -> Result<Layout, LayoutError> {
        match ty {
            Type::Scalar(s) => self.scalar(*s),
            Type::String | Type::Array(_) => Ok(Layout::new(4 * self.word, self.word)),
            Type::Optional(inner) => {
                let cases = [Vec::new(), vec![(**inner).clone()]];
                let niche = self.enum_niche_payload(false, &cases);
                self.enum_layout_of_cases(false, &cases, niche)
            }
            Type::Result(ok, err) => {
                let cases = [vec![(**ok).clone()], vec![(**err).clone()]];
                let niche = self.enum_niche_payload(false, &cases);
                self.enum_layout_of_cases(false, &cases, niche)
            }
            Type::Tuple(items) => Ok(self.aggregate(items)?.0),
            Type::TraitObject(_) | Type::Function { .. } | Type::BorrowFunction { .. } => {
                Ok(Layout::new(2 * self.word, self.word))
            }
            Type::Nominal { name, args } => self.nominal(name, args),
            Type::Generic(name) => Err(LayoutError::Unrepresentable(format!(
                "generic parameter `{name}` has no concrete layout"
            ))),
        }
    }

    fn scalar(&self, s: Scalar) -> Result<Layout, LayoutError> {
        let l = match s {
            Scalar::I8 | Scalar::U8 | Scalar::Bool => Layout::new(1, 1),
            Scalar::I16 | Scalar::U16 => Layout::new(2, 2),
            Scalar::I32 | Scalar::U32 | Scalar::F32 => Layout::new(4, 4),
            Scalar::I64 | Scalar::U64 | Scalar::F64 => Layout::new(8, 8),
            Scalar::I128 | Scalar::U128 => Layout::new(16, 16),
            Scalar::ISize | Scalar::USize | Scalar::RawPtr => Layout::new(self.word, self.word),
            Scalar::Void => {
                return Err(LayoutError::Unrepresentable(
                    "`Void` has no layout".to_owned(),
                ));
            }
        };
        Ok(l)
    }

    fn nominal(&self, name: &str, args: &[Type]) -> Result<Layout, LayoutError> {
        let decl = self
            .registry
            .get(name)
            .ok_or_else(|| LayoutError::UnknownType(name.to_owned()))?;
        let generics = decl.generics();
        if args.len() != generics.len() {
            return Err(LayoutError::ArgCountMismatch {
                name: name.to_owned(),
                expected: generics.len(),
                got: args.len(),
            });
        }
        let subst: BTreeMap<String, Type> =
            generics.iter().cloned().zip(args.iter().cloned()).collect();
        match decl {
            Decl::Struct(s) => self.struct_layout(s, &subst),
            Decl::Enum(e) => self.enum_layout(e, &subst),
        }
    }

    fn struct_layout(
        &self,
        s: &StructDecl,
        subst: &BTreeMap<String, Type>,
    ) -> Result<Layout, LayoutError> {
        let Some(fields) = &s.fields else {
            // Opaque struct: one non-null pointer (section 21.5).
            return Ok(Layout::new(self.word, self.word));
        };
        let tys: Vec<Type> = fields.iter().map(|f| f.ty.substitute(subst)).collect();
        Ok(self.aggregate(&tys)?.0)
    }

    /// Place fields in order (section 21.2). Returns the aggregate layout and
    /// each field's placement.
    ///
    /// # Errors
    /// Returns [`LayoutError`] when any field type has no layout.
    pub fn aggregate(&self, fields: &[Type]) -> Result<(Layout, Vec<FieldPlacement>), LayoutError> {
        let mut offset = 0u64;
        let mut align = 1u64;
        let mut placements = Vec::with_capacity(fields.len());
        for ty in fields {
            let l = self.layout_of(ty)?;
            offset = round_up(offset, l.align);
            placements.push(FieldPlacement {
                name: String::new(),
                offset,
                layout: l,
            });
            offset += l.size;
            align = align.max(l.align);
        }
        let size = round_up(offset, align);
        Ok((Layout::new(size, align), placements))
    }

    /// Named field placements for a concrete struct, offsets included.
    ///
    /// # Errors
    /// Returns [`LayoutError`] when `name` is unknown, is not a struct, or the
    /// generic argument count is wrong.
    pub fn struct_fields(
        &self,
        name: &str,
        args: &[Type],
    ) -> Result<Vec<FieldPlacement>, LayoutError> {
        let decl = self
            .registry
            .get(name)
            .ok_or_else(|| LayoutError::UnknownType(name.to_owned()))?;
        let Decl::Struct(s) = decl else {
            return Err(LayoutError::Unrepresentable(format!(
                "`{name}` is not a struct"
            )));
        };
        let generics = decl.generics();
        if args.len() != generics.len() {
            return Err(LayoutError::ArgCountMismatch {
                name: name.to_owned(),
                expected: generics.len(),
                got: args.len(),
            });
        }
        let subst: BTreeMap<String, Type> =
            generics.iter().cloned().zip(args.iter().cloned()).collect();
        let Some(fields) = &s.fields else {
            return Ok(Vec::new());
        };
        let tys: Vec<Type> = fields.iter().map(|f| f.ty.substitute(&subst)).collect();
        let (_, mut placements) = self.aggregate(&tys)?;
        for (p, f) in placements.iter_mut().zip(fields) {
            p.name.clone_from(&f.name);
        }
        Ok(placements)
    }

    /// The cases of `ty` when it is an enum (`Optional`, `Result` or a
    /// nominal enum): its tag, where payloads start, and each case's payload
    /// placements relative to the enum's start. `None` for any other type.
    ///
    /// # Errors
    /// Returns [`LayoutError`] when a payload has no layout.
    pub fn enum_shape(&self, ty: &Type) -> Result<Option<EnumShape>, LayoutError> {
        let (open, cases) = match ty {
            Type::Optional(inner) => (false, vec![Vec::new(), vec![(**inner).clone()]]),
            Type::Result(ok, err) => (false, vec![vec![(**ok).clone()], vec![(**err).clone()]]),
            Type::Nominal { name, args } => match self.registry.get(name) {
                Some(Decl::Enum(e)) => {
                    let subst: BTreeMap<String, Type> = e
                        .generics
                        .iter()
                        .cloned()
                        .zip(args.iter().cloned())
                        .collect();
                    let cases = e
                        .cases
                        .iter()
                        .map(|c| c.payload.iter().map(|t| t.substitute(&subst)).collect())
                        .collect();
                    (e.open, cases)
                }
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        let niche = self.enum_niche_payload(open, &cases).is_some();
        let tag = if niche {
            Layout::new(0, 1)
        } else {
            tag_layout(open, cases.len())
        };
        let mut payload_align = 1u64;
        let mut placed = Vec::with_capacity(cases.len());
        for payload in &cases {
            let (layout, placements) = self.aggregate(payload)?;
            payload_align = payload_align.max(layout.align);
            placed.push((payload.clone(), placements));
        }
        let payload_offset = if niche {
            0
        } else {
            round_up(tag.size, payload_align)
        };
        let cases = placed
            .into_iter()
            .map(|(types, placements)| {
                types
                    .into_iter()
                    .zip(placements)
                    .map(|(ty, placement)| (ty, payload_offset + placement.offset))
                    .collect()
            })
            .collect();
        Ok(Some(EnumShape {
            tag_size: tag.size,
            niche,
            payload_offset,
            cases,
        }))
    }

    fn enum_layout(
        &self,
        e: &EnumDecl,
        subst: &BTreeMap<String, Type>,
    ) -> Result<Layout, LayoutError> {
        let cases: Vec<Vec<Type>> = e
            .cases
            .iter()
            .map(|c| c.payload.iter().map(|t| t.substitute(subst)).collect())
            .collect();
        let niche = self.enum_niche_payload(e.open, &cases);
        self.enum_layout_of_cases(e.open, &cases, niche)
    }

    /// If the enum qualifies for the pointer niche (section 21.3.4), return the
    /// single payload type whose first word is the niche.
    fn enum_niche_payload(&self, open: bool, cases: &[Vec<Type>]) -> Option<Type> {
        if open || cases.len() != 2 {
            return None;
        }
        let mut empty = 0;
        let mut single = None;
        for payload in cases {
            match payload.len() {
                0 => empty += 1,
                1 => single = Some(payload[0].clone()),
                _ => return None,
            }
        }
        if empty != 1 {
            return None;
        }
        let candidate = single?;
        if self.is_niche_carrier(&candidate) {
            Some(candidate)
        } else {
            None
        }
    }

    fn is_niche_carrier(&self, ty: &Type) -> bool {
        match ty {
            Type::TraitObject(_) | Type::Function { .. } | Type::BorrowFunction { .. } => true,
            Type::Nominal { name, .. } => matches!(
                self.registry.get(name),
                Some(Decl::Struct(s)) if s.is_opaque()
            ),
            _ => false,
        }
    }

    fn enum_layout_of_cases(
        &self,
        open: bool,
        cases: &[Vec<Type>],
        niche: Option<Type>,
    ) -> Result<Layout, LayoutError> {
        if let Some(payload) = niche {
            return self.layout_of(&payload);
        }
        let tag = tag_layout(open, cases.len());
        let mut max_payload_size = 0u64;
        let mut max_payload_align = 1u64;
        for payload in cases {
            let (l, _) = self.aggregate(payload)?;
            max_payload_size = max_payload_size.max(l.size);
            max_payload_align = max_payload_align.max(l.align);
        }
        if cases.iter().all(Vec::is_empty) {
            return Ok(tag);
        }
        let payload_offset = round_up(tag.size, max_payload_align);
        let align = tag.align.max(max_payload_align);
        let size = round_up(payload_offset + max_payload_size, align);
        Ok(Layout::new(size, align))
    }
}

fn tag_layout(open: bool, case_count: usize) -> Layout {
    if open || case_count > 65_536 {
        Layout::new(4, 4)
    } else if case_count > 256 {
        Layout::new(2, 2)
    } else {
        Layout::new(1, 1)
    }
}
