//! Types, bindings and address spaces in textual KSIR.

use super::Parser;
use super::lexer::Token;
use crate::Span;
use crate::arena::Handle;
use crate::types::{
    Access, AddressSpace, ArraySize, Binding, BuiltIn, ImageClass, ImageDim, Interpolation,
    Sampling, Scalar, ScalarKind, StorageFormat, Type, TypeInner,
};

impl Parser {
    /// An optional binding: `@builtin(b)` or `@location(n) ...`.
    pub(crate) fn binding(&mut self) -> Result<Option<Binding>, String> {
        if self.peek() != Some(&Token::At) {
            return Ok(None);
        }
        let mut binding = None;
        while self.peek() == Some(&Token::At) && matches!(self.peek_at(1), Some(Token::Ident(_))) {
            self.at += 1;
            let word = self.ident()?;
            self.expect_punct('(')?;
            let value = self.ident()?;
            self.expect_punct(')')?;
            match (word.as_str(), &mut binding) {
                ("builtin", None) => {
                    let builtin = BuiltIn::from_name(&value)
                        .map_or_else(|| self.error(format!("unknown built-in `{value}`")), Ok)?;
                    binding = Some(Binding::BuiltIn(builtin));
                }
                ("location", None) => {
                    let location = match value.as_str() {
                        "auto" => None,
                        number => Some(number.parse().or_else(|_| self.error("bad location"))?),
                    };
                    binding = Some(Binding::Location {
                        location,
                        interpolation: None,
                        sampling: None,
                        blend_src: None,
                    });
                }
                ("interpolate", Some(Binding::Location { interpolation, .. })) => {
                    *interpolation = Some(match value.as_str() {
                        "perspective" => Interpolation::Perspective,
                        "linear" => Interpolation::Linear,
                        "flat" => Interpolation::Flat,
                        _ => return self.error("unknown interpolation"),
                    });
                }
                ("sampling", Some(Binding::Location { sampling, .. })) => {
                    *sampling = Some(match value.as_str() {
                        "center" => Sampling::Center,
                        "centroid" => Sampling::Centroid,
                        "sample" => Sampling::Sample,
                        _ => return self.error("unknown sampling"),
                    });
                }
                ("blend_src", Some(Binding::Location { blend_src, .. })) => {
                    *blend_src = Some(value.parse().or_else(|_| self.error("bad blend source"))?);
                }
                _ => return self.error(format!("unexpected `@{word}`")),
            }
        }
        Ok(binding)
    }

    pub(crate) fn intern(&mut self, inner: TypeInner) -> Handle<Type> {
        self.module
            .types
            .insert(Type { name: None, inner }, Span::default())
    }

    pub(crate) fn scalar(&mut self) -> Result<Scalar, String> {
        let name = self.ident()?;
        Scalar::from_text(&name).map_or_else(|| self.error(format!("unknown scalar `{name}`")), Ok)
    }

    pub(crate) fn space(&mut self) -> Result<AddressSpace, String> {
        let word = self.ident()?;
        Ok(match word.as_str() {
            "function" => AddressSpace::Function,
            "private" => AddressSpace::Private,
            "workgroup" => AddressSpace::Workgroup,
            "uniform" => AddressSpace::Uniform,
            "handle" => AddressSpace::Handle,
            "immediate" => AddressSpace::Immediate,
            "storage" => {
                self.expect_punct('<')?;
                let access = self.access()?;
                self.expect_punct('>')?;
                AddressSpace::Storage { access }
            }
            other => return self.error(format!("unknown space `{other}`")),
        })
    }

    fn access(&mut self) -> Result<Access, String> {
        let mut words = Vec::new();
        while let Some(Token::Ident(word)) = self.peek().cloned() {
            self.at += 1;
            words.push(word);
        }
        Access::from_words(words.iter().map(String::as_str))
            .map_or_else(|| self.error("unknown access"), Ok)
    }

    pub(crate) fn ty(&mut self) -> Result<Handle<Type>, String> {
        if self.eat_punct('%') {
            let name = self.ident()?;
            return self
                .module
                .types
                .iter()
                .find(|(_, ty)| ty.name.as_deref() == Some(name.as_str()))
                .map(|(handle, _)| handle)
                .map_or_else(|| self.error(format!("unknown type `%{name}`")), Ok);
        }
        let word = self.ident()?;
        let inner = match word.as_str() {
            "atomic" => {
                self.expect_punct('<')?;
                let scalar = self.scalar()?;
                self.expect_punct('>')?;
                TypeInner::Atomic(scalar)
            }
            "ptr" => {
                self.expect_punct('<')?;
                let space = self.space()?;
                self.expect_punct(',')?;
                let base = self.ty()?;
                self.expect_punct('>')?;
                TypeInner::Pointer { base, space }
            }
            "array" => {
                self.expect_punct('<')?;
                let base = self.ty()?;
                self.expect_punct(',')?;
                let size =
                    if self.peek() == Some(&Token::At) {
                        let name = self.at_name()?;
                        let found = self
                            .module
                            .overrides
                            .iter()
                            .find(|(_, item)| item.name == name)
                            .map(|(handle, _)| handle);
                        ArraySize::Override(found.map_or_else(
                            || self.error(format!("unknown override `@{name}`")),
                            Ok,
                        )?)
                    } else if self.eat_word("runtime") {
                        ArraySize::Runtime
                    } else {
                        ArraySize::Constant(self.number()?)
                    };
                self.expect_punct(',')?;
                self.expect_word("stride")?;
                let stride = self.number()?;
                self.expect_punct('>')?;
                TypeInner::Array { base, size, stride }
            }
            "sampler" => {
                let comparison = self.eat_punct('<');
                if comparison {
                    self.expect_word("comparison")?;
                    self.expect_punct('>')?;
                }
                TypeInner::Sampler { comparison }
            }
            "image" => self.image()?,
            _ if word.starts_with("vec") => {
                let size = word[3..]
                    .parse()
                    .or_else(|_| self.error(format!("unknown type `{word}`")))?;
                self.expect_punct('<')?;
                let scalar = self.scalar()?;
                self.expect_punct('>')?;
                TypeInner::Vector { size, scalar }
            }
            _ if word.starts_with("mat") && word.len() == 6 => {
                let columns = word[3..4].parse().or_else(|_| self.error("bad matrix"))?;
                let rows = word[5..6].parse().or_else(|_| self.error("bad matrix"))?;
                self.expect_punct('<')?;
                let scalar = self.scalar()?;
                self.expect_punct('>')?;
                TypeInner::Matrix {
                    columns,
                    rows,
                    scalar,
                }
            }
            _ => TypeInner::Scalar(
                Scalar::from_text(&word)
                    .map_or_else(|| self.error(format!("unknown type `{word}`")), Ok)?,
            ),
        };
        Ok(self.intern(inner))
    }

    fn image(&mut self) -> Result<TypeInner, String> {
        self.expect_punct('<')?;
        let dim = match self.ident()?.as_str() {
            "1d" => ImageDim::D1,
            "2d" => ImageDim::D2,
            "3d" => ImageDim::D3,
            "cube" => ImageDim::Cube,
            other => return self.error(format!("unknown dimension `{other}`")),
        };
        self.expect_punct(',')?;
        let arrayed = self.eat_word("arrayed");
        if arrayed {
            self.expect_punct(',')?;
        }
        let class = match self.ident()?.as_str() {
            "sampled" => {
                self.expect_punct('<')?;
                let kind = self.ident()?;
                let kind =
                    ScalarKind::from_name(&kind).map_or_else(|| self.error("unknown kind"), Ok)?;
                let multisampled = self.eat_punct(',') && self.eat_word("multisampled");
                self.expect_punct('>')?;
                ImageClass::Sampled { kind, multisampled }
            }
            "depth" => {
                let multisampled = self.eat_punct('<');
                if multisampled {
                    self.expect_word("multisampled")?;
                    self.expect_punct('>')?;
                }
                ImageClass::Depth { multisampled }
            }
            "storage" => {
                self.expect_punct('<')?;
                let format = self.ident()?;
                let format = StorageFormat::from_name(&format)
                    .map_or_else(|| self.error(format!("unknown format `{format}`")), Ok)?;
                self.expect_punct(',')?;
                let access = self.access()?;
                self.expect_punct('>')?;
                ImageClass::Storage { format, access }
            }
            other => return self.error(format!("unknown image class `{other}`")),
        };
        self.expect_punct('>')?;
        Ok(TypeInner::Image {
            dim,
            arrayed,
            class,
        })
    }
}
