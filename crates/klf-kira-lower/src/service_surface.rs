//! The service wrappers the KCore frontend makes available to definitions.

use crate::ty::FeType;
use klf_services::KernelServiceManifest;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KCoreServiceType {
    Integer { bits: u16, signed: bool },
    Float { bits: u16 },
    Bool,
    Pointer,
    String,
    Handle(String),
    Void,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KCoreService {
    pub name: String,
    pub symbol: String,
    pub parameters: Vec<KCoreServiceType>,
    pub result: KCoreServiceType,
}

fn surface_type(ty: FeType) -> Option<KCoreServiceType> {
    Some(match ty {
        FeType::Int { bits, signed } => KCoreServiceType::Integer { bits, signed },
        FeType::Float { bits } => KCoreServiceType::Float { bits },
        FeType::Bool => KCoreServiceType::Bool,
        FeType::Pointer => KCoreServiceType::Pointer,
        FeType::Void => KCoreServiceType::Void,
        FeType::Struct(name) if name.as_str() == "String" => KCoreServiceType::String,
        FeType::DefinitionHandle(handle) => KCoreServiceType::Handle(format!("{handle:?}")),
        _ => return None,
    })
}

/// Returns exactly the manifest-backed wrappers accepted by KCore.
///
/// # Errors
/// Returns an error if a wrapper uses a type this surface cannot represent.
pub fn kcore_services(manifest: &KernelServiceManifest) -> Result<Vec<KCoreService>, String> {
    let mut wrappers = Vec::new();
    for service in manifest.services() {
        let Some(name) = crate::builtins::wrapper_name(&service.name) else {
            continue;
        };
        let (params, result) = crate::builtins::wrapper_types(service)
            .ok_or_else(|| format!("missing wrapper types for {}", service.name))?;
        let parameters = params
            .into_iter()
            .map(|ty| {
                surface_type(ty)
                    .ok_or_else(|| format!("unsupported parameter type for {}", service.name))
            })
            .collect::<Result<_, _>>()?;
        let result = surface_type(result)
            .ok_or_else(|| format!("unsupported result type for {}", service.name))?;
        wrappers.push(KCoreService {
            name,
            symbol: service.symbol.clone(),
            parameters,
            result,
        });
    }
    Ok(wrappers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use klf_services::{KERNEL_SERVICE_MANIFEST, ServiceType};

    #[test]
    fn kcore_service_surface_matches_manifest_shapes() {
        let manifest = KernelServiceManifest::parse(KERNEL_SERVICE_MANIFEST).expect("manifest");
        let wrappers = kcore_services(&manifest).expect("wrapper surface");
        assert!(!wrappers.is_empty());
        for wrapper in wrappers {
            let service = manifest
                .services()
                .iter()
                .find(|service| service.symbol == wrapper.symbol)
                .expect("service");
            let mut parameters = service.signature.parameters.iter();
            for parameter in &wrapper.parameters {
                let abi = parameters
                    .next()
                    .unwrap_or_else(|| panic!("{} has too few ABI parameters", wrapper.name));
                match parameter {
                    KCoreServiceType::String => {
                        assert_eq!(*abi, ServiceType::Pointer, "{}", wrapper.name);
                        assert_eq!(
                            parameters.next(),
                            Some(&ServiceType::Integer {
                                bits: 64,
                                signed: true
                            }),
                            "{}",
                            wrapper.name
                        );
                    }
                    KCoreServiceType::Pointer => {
                        assert_eq!(*abi, ServiceType::Pointer, "{}", wrapper.name)
                    }
                    KCoreServiceType::Handle(_) => assert_eq!(
                        *abi,
                        ServiceType::Integer {
                            bits: 64,
                            signed: true
                        },
                        "{}",
                        wrapper.name
                    ),
                    KCoreServiceType::Integer { .. } | KCoreServiceType::Bool => assert!(
                        matches!(abi, ServiceType::Integer { .. }),
                        "{}: {abi:?}",
                        wrapper.name
                    ),
                    KCoreServiceType::Float { .. } => assert!(
                        matches!(abi, ServiceType::Float { .. }),
                        "{}: {abi:?}",
                        wrapper.name
                    ),
                    KCoreServiceType::Void => panic!("void parameter: {}", wrapper.name),
                }
            }
            assert!(
                parameters.next().is_none(),
                "{} has extra ABI parameters",
                wrapper.name
            );
        }
    }
}
