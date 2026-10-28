//! Typed entries from the kernel service ABI manifest.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceType {
    Integer { bits: u16, signed: bool },
    Float { bits: u16 },
    Pointer,
    Void,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceSignature {
    pub parameters: Vec<ServiceType>,
    pub result: ServiceType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KernelService {
    pub name: String,
    pub symbol: String,
    pub signature: ServiceSignature,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceManifestError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ServiceManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ServiceManifestError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KernelServiceManifest {
    services: Vec<KernelService>,
}

impl KernelServiceManifest {
    /// Parses the whitespace-delimited manifest format and its inline comments.
    ///
    /// # Errors
    /// Returns the source line and reason for malformed entries or duplicate
    /// service names and linker symbols.
    pub fn parse(text: &str) -> Result<Self, ServiceManifestError> {
        let mut services = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            let content = line
                .split_once('#')
                .map_or(line, |(content, _)| content)
                .trim();
            if content.is_empty() {
                continue;
            }
            let mut fields = content.split_whitespace();
            let Some(name) = fields.next() else {
                continue;
            };
            let Some(symbol) = fields.next() else {
                return Err(error(line_number, "service is missing its linker symbol"));
            };
            let signature = fields.collect::<Vec<_>>().join(" ");
            if signature.is_empty() {
                return Err(error(line_number, "service is missing its signature"));
            }
            if services
                .iter()
                .any(|service: &KernelService| service.name == name)
            {
                return Err(error(line_number, "duplicate service name"));
            }
            if services.iter().any(|service| service.symbol == symbol) {
                return Err(error(line_number, "duplicate service symbol"));
            }
            services.push(KernelService {
                name: name.to_owned(),
                symbol: symbol.to_owned(),
                signature: parse_signature(&signature, line_number)?,
            });
        }
        Ok(Self { services })
    }

    #[must_use]
    pub fn services(&self) -> &[KernelService] {
        &self.services
    }

    #[must_use]
    pub fn service(&self, name: &str) -> Option<&KernelService> {
        self.services.iter().find(|service| service.name == name)
    }
}

pub const KERNEL_SERVICE_MANIFEST: &str = include_str!("../../../kernel/services.manifest");

fn parse_signature(text: &str, line: usize) -> Result<ServiceSignature, ServiceManifestError> {
    let Some((parameters, result)) = text.split_once("->") else {
        return Err(error(line, "signature is missing `->`"));
    };
    let parameters = parameters.trim();
    if !parameters.starts_with('(') || !parameters.ends_with(')') {
        return Err(error(line, "signature parameters must be parenthesized"));
    }
    let parameters = &parameters[1..parameters.len() - 1];
    let parameters = if parameters.trim().is_empty() {
        Vec::new()
    } else {
        parameters
            .split(',')
            .map(|parameter| parse_type(parameter.trim(), line))
            .collect::<Result<_, _>>()?
    };
    Ok(ServiceSignature {
        parameters,
        result: parse_type(result.trim(), line)?,
    })
}

fn parse_type(text: &str, line: usize) -> Result<ServiceType, ServiceManifestError> {
    if text == "void" {
        return Ok(ServiceType::Void);
    }
    if text == "ptr" || text == "pointer" {
        return Ok(ServiceType::Pointer);
    }
    let Some(kind) = text.chars().next() else {
        return Err(error(line, "empty service type"));
    };
    let bits = &text[kind.len_utf8()..];
    let Ok(bits) = bits.parse::<u16>() else {
        return Err(error(line, format!("invalid service type `{text}`")));
    };
    match kind {
        'i' => Ok(ServiceType::Integer { bits, signed: true }),
        'u' => Ok(ServiceType::Integer {
            bits,
            signed: false,
        }),
        'f' => Ok(ServiceType::Float { bits }),
        _ => Err(error(line, format!("invalid service type `{text}`"))),
    }
}

fn error(line: usize, message: impl Into<String>) -> ServiceManifestError {
    ServiceManifestError {
        line,
        message: message.into(),
    }
}
