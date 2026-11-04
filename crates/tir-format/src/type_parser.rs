use std::fmt;

use tir_diag::Span;
use tir_types::{
    Abi, AddressSpace, ClosureType, Convention, CooperativeMatrixUse, EntryKind, FloatType,
    FunctionType, MatrixScope, ParameterType, TextureAccess, TextureDimension, Type, TypeError,
};

use crate::text::{TextError, Token, TokenKind, lex};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeParseError {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for TypeParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TypeParseError {}

impl From<TextError> for TypeParseError {
    fn from(error: TextError) -> Self {
        Self {
            span: error.span,
            message: error.message.to_owned(),
        }
    }
}

/// Parses one TIR type using Appendix A.1 and validates its structural limits.
pub fn parse_type(source: &str) -> Result<Type, TypeParseError> {
    parse_type_mode(source, false)
}

/// Parses one TVISA value type, including TVISA's `code` value and type subset.
pub fn parse_tvisa_type(source: &str) -> Result<Type, TypeParseError> {
    let ty = parse_type_mode(source, true)?;
    if !is_tvisa_type(&ty) {
        return Err(TypeParseError {
            span: Span::new(0, source.len()),
            message: "type is not permitted in TVISA".to_owned(),
        });
    }
    Ok(ty)
}

fn parse_type_mode(source: &str, allow_code: bool) -> Result<Type, TypeParseError> {
    let tokens = lex(source).map_err(TypeParseError::from)?;
    let mut parser = Parser {
        tokens,
        cursor: 0,
        allow_code,
        type_depth: 0,
    };
    let ty = parser.parse_type()?;
    if !matches!(parser.peek().kind, TokenKind::End) {
        return Err(parser.error("unexpected token after type"));
    }
    let validation = if allow_code {
        ty.validate_tvisa()
    } else {
        ty.validate()
    };
    validation.map_err(|error| parser.type_error(error))?;
    Ok(ty)
}

fn is_tvisa_type(ty: &Type) -> bool {
    match ty {
        Type::Integer(bits) => matches!(*bits, 1 | 8 | 16 | 32 | 64 | 128),
        Type::Float(FloatType::F8E4M3 | FloatType::F8E5M2 | FloatType::E8M0) => false,
        Type::Float(_)
        | Type::Pointer(_)
        | Type::Code
        | Type::Sampler
        | Type::AccelerationStructure
        | Type::RayQuery
        | Type::HitObject => true,
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => {
            is_tvisa_element_type(element)
        }
        Type::Texture { component, .. } => is_tvisa_type(component),
        Type::Tensor { element, .. } => is_tvisa_element_type(element),
        Type::VisibleFunctionTable(_) => true,
        Type::CooperativeMatrix { element, .. } => is_tvisa_element_type(element),
        Type::Unit
        | Type::Never
        | Type::Tuple(_)
        | Type::Array { .. }
        | Type::Nominal { .. }
        | Type::Generic(_)
        | Type::Reference(_)
        | Type::Unique(_)
        | Type::Weak(_)
        | Type::Address { .. }
        | Type::Function(_)
        | Type::Closure(_)
        | Type::Coroutine { .. }
        | Type::Frame(_)
        | Type::Entry { .. } => false,
    }
}

fn is_tvisa_element_type(ty: &Type) -> bool {
    match ty {
        Type::Integer(bits) => matches!(*bits, 1 | 8 | 16 | 32 | 64 | 128),
        Type::Float(_) | Type::Pointer(tir_types::AddressSpace::Generic) => true,
        _ => false,
    }
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    allow_code: bool,
    type_depth: usize,
}

impl Parser {
    fn parse_type(&mut self) -> Result<Type, TypeParseError> {
        if self.type_depth >= 256 {
            return Err(self.type_error(TypeError::NestingTooDeep));
        }
        self.type_depth += 1;
        let result = self.parse_type_inner();
        self.type_depth -= 1;
        result
    }

    fn parse_type_inner(&mut self) -> Result<Type, TypeParseError> {
        match self.next().kind {
            TokenKind::Punctuation('%') => self.parse_nominal(),
            TokenKind::Punctuation('$') => Ok(Type::Generic(self.take_atom()?)),
            TokenKind::Punctuation('*') => {
                let pointee = Box::new(self.parse_type()?);
                let space = if self.eat_punctuation('<') {
                    let space = self.parse_address_space()?;
                    self.expect_punctuation('>')?;
                    Some(space)
                } else {
                    None
                };
                Ok(Type::Address { pointee, space })
            }
            TokenKind::Punctuation('(') => self.parse_tuple(),
            TokenKind::Punctuation('[') => self.parse_array(),
            TokenKind::Atom(atom) => self.parse_atom_type(&atom),
            _ => Err(self.error("expected a TIR type")),
        }
    }

    fn parse_atom_type(&mut self, atom: &str) -> Result<Type, TypeParseError> {
        if let Some(bits) = atom.strip_prefix('i') {
            if crate::numeric_literals::parse_unsigned_integer(bits).is_some() {
                return Ok(Type::Integer(self.parse_u16(bits)?));
            }
        }
        if let Some(float) = parse_float(atom) {
            return Ok(Type::Float(float));
        }
        match atom {
            "unit" => Ok(Type::Unit),
            "never" => Ok(Type::Never),
            "ptr" => {
                if self.eat_punctuation('<') {
                    let space = self.parse_address_space()?;
                    self.expect_punctuation('>')?;
                    Ok(Type::Pointer(space))
                } else {
                    Ok(Type::Pointer(AddressSpace::Generic))
                }
            }
            "code" if self.allow_code => Ok(Type::Code),
            "vec" => self.parse_fixed_vector(),
            "svec" => {
                self.expect_punctuation('<')?;
                let element = Box::new(self.parse_type()?);
                self.expect_punctuation('>')?;
                Ok(Type::ScalableVector(element))
            }
            "ref" => Ok(Type::Reference(Box::new(self.parse_type()?))),
            "uniq" => Ok(Type::Unique(Box::new(self.parse_type()?))),
            "weak" => Ok(Type::Weak(Box::new(self.parse_type()?))),
            "fn" => Ok(Type::Function(self.parse_signature()?)),
            "closure" => self.parse_closure(false),
            "borrow" => {
                self.expect_atom("closure")?;
                self.parse_closure(true)
            }
            "coro" => self.parse_coroutine(),
            "frame" => {
                self.expect_punctuation('@')?;
                Ok(Type::Frame(self.parse_path()?))
            }
            "texture" => self.parse_texture(),
            "sampler" => Ok(Type::Sampler),
            "accel" => Ok(Type::AccelerationStructure),
            "vft" => {
                self.expect_punctuation('<')?;
                self.expect_atom("fn")?;
                let signature = self.parse_signature()?;
                self.expect_punctuation('>')?;
                Ok(Type::VisibleFunctionTable(Box::new(signature)))
            }
            "tensor" => self.parse_tensor(),
            "rayquery" => Ok(Type::RayQuery),
            "hitobject" => Ok(Type::HitObject),
            "coop" => self.parse_cooperative_matrix(),
            "entry" => self.parse_entry(),
            _ => Err(self.error("unknown TIR type")),
        }
    }

    fn parse_nominal(&mut self) -> Result<Type, TypeParseError> {
        let path = self.parse_path()?;
        let mut arguments = Vec::new();
        if self.eat_punctuation('<') {
            arguments = self.parse_type_list('>')?;
            self.expect_punctuation('>')?;
        }
        Ok(Type::Nominal { path, arguments })
    }

    fn parse_tuple(&mut self) -> Result<Type, TypeParseError> {
        let elements = self.parse_type_list(')')?;
        self.expect_punctuation(')')?;
        Ok(Type::Tuple(elements))
    }

    fn parse_array(&mut self) -> Result<Type, TypeParseError> {
        let count_text = self.take_atom()?;
        let count = self.parse_u64(&count_text)?;
        self.expect_atom("x")?;
        let element = Box::new(self.parse_type()?);
        self.expect_punctuation(']')?;
        Ok(Type::Array { count, element })
    }

    fn parse_fixed_vector(&mut self) -> Result<Type, TypeParseError> {
        self.expect_punctuation('<')?;
        let lanes_text = self.take_atom()?;
        let lanes = self.parse_u32(&lanes_text)?;
        self.expect_atom("x")?;
        let element = Box::new(self.parse_type()?);
        self.expect_punctuation('>')?;
        Ok(Type::FixedVector { lanes, element })
    }

    fn parse_signature(&mut self) -> Result<FunctionType, TypeParseError> {
        self.expect_punctuation('(')?;
        let mut parameters = Vec::new();
        if !self.eat_punctuation(')') {
            loop {
                parameters.push(self.parse_parameter()?);
                if self.eat_punctuation(')') {
                    break;
                }
                self.expect_punctuation(',')?;
            }
        }
        if !self.eat_arrow() {
            return Err(self.error("expected -> in function type"));
        }
        let result = Box::new(self.parse_type()?);
        let failure = if self.eat_atom("fails") {
            Some(Box::new(self.parse_type()?))
        } else {
            None
        };
        let asynchronous = self.eat_atom("async");
        let abi = if self.eat_atom("abi") {
            self.expect_punctuation('(')?;
            let abi = match self.take_atom()?.as_str() {
                "ridl" => Abi::Ridl,
                "c" => Abi::C,
                _ => return Err(self.error("unknown function ABI")),
            };
            self.expect_punctuation(')')?;
            abi
        } else {
            Abi::Ridl
        };
        let foreign = if self.eat_atom("foreign") {
            true
        } else if self.eat_punctuation('[') {
            self.expect_atom("foreign")?;
            self.expect_punctuation(']')?;
            true
        } else {
            false
        };
        Ok(FunctionType {
            parameters,
            result,
            failure,
            asynchronous,
            abi,
            foreign,
        })
    }

    fn parse_parameter(&mut self) -> Result<ParameterType, TypeParseError> {
        let context = self.eat_atom("context");
        let convention = if self.eat_atom("owned") {
            Some(Convention::Owned)
        } else if self.eat_atom("borrow") {
            Some(Convention::Borrow)
        } else if self.eat_atom("borrowmut") {
            Some(Convention::BorrowMut)
        } else {
            None
        };
        Ok(ParameterType {
            context,
            convention,
            ty: self.parse_type()?,
        })
    }

    fn parse_closure(&mut self, borrowed: bool) -> Result<Type, TypeParseError> {
        Ok(Type::Closure(ClosureType {
            borrowed,
            signature: self.parse_signature()?,
        }))
    }

    fn parse_coroutine(&mut self) -> Result<Type, TypeParseError> {
        self.expect_punctuation('<')?;
        let yield_type = Box::new(self.parse_type()?);
        self.expect_punctuation(',')?;
        let resume_type = Box::new(self.parse_type()?);
        self.expect_punctuation(',')?;
        let result_type = Box::new(self.parse_type()?);
        self.expect_punctuation('>')?;
        Ok(Type::Coroutine {
            yield_type,
            resume_type,
            result_type,
        })
    }

    fn parse_texture(&mut self) -> Result<Type, TypeParseError> {
        self.expect_punctuation('<')?;
        let dimension = match self.take_atom()?.as_str() {
            "1d" => TextureDimension::D1,
            "2d" => TextureDimension::D2,
            "3d" => TextureDimension::D3,
            "cube" => TextureDimension::Cube,
            "1darray" => TextureDimension::D1Array,
            "2darray" => TextureDimension::D2Array,
            "cubearray" => TextureDimension::CubeArray,
            _ => return Err(self.error("unknown texture dimension")),
        };
        self.expect_punctuation(',')?;
        let component = Box::new(self.parse_type()?);
        self.expect_punctuation(',')?;
        let access = match self.take_atom()?.as_str() {
            "sample" => TextureAccess::Sample,
            "read" => TextureAccess::Read,
            "write" => TextureAccess::Write,
            "readwrite" => TextureAccess::ReadWrite,
            _ => return Err(self.error("unknown texture access")),
        };
        let multisampled = if self.eat_punctuation(',') {
            self.expect_atom("ms")?;
            true
        } else {
            false
        };
        self.expect_punctuation('>')?;
        Ok(Type::Texture {
            dimension,
            component,
            access,
            multisampled,
        })
    }

    fn parse_tensor(&mut self) -> Result<Type, TypeParseError> {
        self.expect_punctuation('<')?;
        let count_text = self.take_atom()?;
        let count = self.parse_u32(&count_text)?;
        self.expect_atom("x")?;
        let element = Box::new(self.parse_type()?);
        self.expect_punctuation('>')?;
        Ok(Type::Tensor { count, element })
    }

    fn parse_cooperative_matrix(&mut self) -> Result<Type, TypeParseError> {
        self.expect_punctuation('<')?;
        let scope = match self.take_atom()?.as_str() {
            "subgroup" => MatrixScope::Subgroup,
            "threadgroup" => MatrixScope::Threadgroup,
            _ => return Err(self.error("unknown cooperative matrix scope")),
        };
        self.expect_punctuation(',')?;
        let rows_text = self.take_atom()?;
        let rows = self.parse_u32(&rows_text)?;
        self.expect_atom("x")?;
        let columns_text = self.take_atom()?;
        let columns = self.parse_u32(&columns_text)?;
        self.expect_punctuation(',')?;
        let element = Box::new(self.parse_type()?);
        self.expect_punctuation(',')?;
        let use_kind = match self.take_atom()?.as_str() {
            "a" => CooperativeMatrixUse::A,
            "b" => CooperativeMatrixUse::B,
            "acc" => CooperativeMatrixUse::Acc,
            _ => return Err(self.error("unknown cooperative matrix use")),
        };
        self.expect_punctuation('>')?;
        Ok(Type::CooperativeMatrix {
            scope,
            rows,
            columns,
            element,
            use_kind,
        })
    }

    fn parse_entry(&mut self) -> Result<Type, TypeParseError> {
        self.expect_punctuation('<')?;
        let kind = parse_entry_kind(&self.take_atom()?)
            .ok_or_else(|| self.error("unknown GPU entry kind"))?;
        self.expect_punctuation(',')?;
        self.expect_atom("root")?;
        let root = Box::new(self.parse_type()?);
        self.expect_punctuation(',')?;
        self.expect_atom("immediate")?;
        let immediate = Box::new(self.parse_type()?);
        self.expect_punctuation('>')?;
        Ok(Type::Entry {
            kind,
            root,
            immediate,
        })
    }

    fn parse_type_list(&mut self, end: char) -> Result<Vec<Type>, TypeParseError> {
        let mut types = Vec::new();
        if self.eat_punctuation(end) {
            self.cursor -= 1;
            return Ok(types);
        }
        loop {
            types.push(self.parse_type()?);
            if !self.eat_punctuation(',') {
                break;
            }
        }
        Ok(types)
    }

    fn parse_path(&mut self) -> Result<String, TypeParseError> {
        let mut path = self.take_atom()?;
        while self.eat_punctuation('.') {
            path.push('.');
            path.push_str(&self.take_atom()?);
        }
        Ok(path)
    }

    fn parse_address_space(&mut self) -> Result<AddressSpace, TypeParseError> {
        match self.take_atom()?.as_str() {
            "generic" => Ok(AddressSpace::Generic),
            "device" => Ok(AddressSpace::Device),
            "constant" => Ok(AddressSpace::Constant),
            "threadgroup" => Ok(AddressSpace::Threadgroup),
            "thread" => Ok(AddressSpace::Thread),
            "object_payload" => Ok(AddressSpace::ObjectPayload),
            "ray_payload" => Ok(AddressSpace::RayPayload),
            "hit_attribute" => Ok(AddressSpace::HitAttribute),
            "tile" => Ok(AddressSpace::Tile),
            "node_record" => Ok(AddressSpace::NodeRecord),
            _ => Err(self.error("unknown address space")),
        }
    }

    fn take_atom(&mut self) -> Result<String, TypeParseError> {
        match self.next().kind {
            TokenKind::Atom(atom) => Ok(atom),
            _ => Err(self.error("expected identifier or number")),
        }
    }

    fn expect_atom(&mut self, expected: &str) -> Result<(), TypeParseError> {
        if self.eat_atom(expected) {
            Ok(())
        } else {
            Err(self.error("unexpected identifier"))
        }
    }

    fn eat_atom(&mut self, expected: &str) -> bool {
        if matches!(&self.peek().kind, TokenKind::Atom(atom) if atom == expected) {
            self.next();
            true
        } else {
            false
        }
    }

    fn expect_punctuation(&mut self, expected: char) -> Result<(), TypeParseError> {
        if self.eat_punctuation(expected) {
            Ok(())
        } else {
            Err(self.error("unexpected punctuation"))
        }
    }

    fn eat_punctuation(&mut self, expected: char) -> bool {
        if matches!(self.peek().kind, TokenKind::Punctuation(actual) if actual == expected) {
            self.next();
            true
        } else {
            false
        }
    }

    fn eat_arrow(&mut self) -> bool {
        if matches!(self.peek().kind, TokenKind::Arrow) {
            self.next();
            true
        } else {
            false
        }
    }

    fn parse_u16(&mut self, value: &str) -> Result<u16, TypeParseError> {
        crate::numeric_literals::parse_unsigned_integer(value)
            .and_then(|number| u16::try_from(number).ok())
            .ok_or_else(|| self.error("integer width is out of range"))
    }

    fn parse_u32(&mut self, value: &str) -> Result<u32, TypeParseError> {
        crate::numeric_literals::parse_unsigned_integer(value)
            .and_then(|number| u32::try_from(number).ok())
            .ok_or_else(|| self.error("integer is out of range"))
    }

    fn parse_u64(&mut self, value: &str) -> Result<u64, TypeParseError> {
        crate::numeric_literals::parse_unsigned_integer(value)
            .and_then(|number| u64::try_from(number).ok())
            .ok_or_else(|| self.error("integer is out of range"))
    }

    fn next(&mut self) -> Token {
        self.skip_newlines();
        let token = self.tokens[self.cursor].clone();
        if !matches!(token.kind, TokenKind::End) {
            self.cursor += 1;
        }
        token
    }

    fn peek(&mut self) -> &Token {
        self.skip_newlines();
        &self.tokens[self.cursor]
    }

    fn skip_newlines(&mut self) {
        while matches!(self.tokens[self.cursor].kind, TokenKind::Newline) {
            self.cursor += 1;
        }
    }

    fn error(&mut self, message: &str) -> TypeParseError {
        TypeParseError {
            span: self.peek().span,
            message: message.to_owned(),
        }
    }

    fn type_error(&mut self, error: TypeError) -> TypeParseError {
        TypeParseError {
            span: self.peek().span,
            message: error.to_string(),
        }
    }
}

fn parse_float(value: &str) -> Option<FloatType> {
    Some(match value {
        "f16" => FloatType::F16,
        "bf16" => FloatType::Bf16,
        "f32" => FloatType::F32,
        "f64" => FloatType::F64,
        "f128" => FloatType::F128,
        "f8e4m3" => FloatType::F8E4M3,
        "f8e5m2" => FloatType::F8E5M2,
        "f6e2m3" => FloatType::F6E2M3,
        "f6e3m2" => FloatType::F6E3M2,
        "f4e2m1" => FloatType::F4E2M1,
        "e8m0" => FloatType::E8M0,
        _ => return None,
    })
}

fn parse_entry_kind(value: &str) -> Option<EntryKind> {
    Some(match value {
        "compute" => EntryKind::Compute,
        "object" => EntryKind::Object,
        "mesh" => EntryKind::Mesh,
        "vertex" => EntryKind::Vertex,
        "fragment" => EntryKind::Fragment,
        "tile" => EntryKind::Tile,
        "raygen" => EntryKind::Raygen,
        "intersection" => EntryKind::Intersection,
        "anyhit" => EntryKind::Anyhit,
        "closesthit" => EntryKind::Closesthit,
        "miss" => EntryKind::Miss,
        "callable" => EntryKind::Callable,
        "node" => EntryKind::Node,
        _ => return None,
    })
}
