//! Kira wrappers for the service names and types accepted by KCore.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use klf_kira_lower::{KCoreServiceType as T, kcore_services};
use klf_services::{KERNEL_SERVICE_MANIFEST, KernelServiceManifest, ServiceType};

/// Generates Kira wrappers for the service calls available to KCore.
///
/// # Errors
/// Returns an error if a service signature cannot be represented.
pub fn generate() -> Result<String, String> {
    let manifest =
        KernelServiceManifest::parse(KERNEL_SERVICE_MANIFEST).map_err(|error| error.to_string())?;
    let wrappers = kcore_services(&manifest)?;
    let mut handles = BTreeSet::new();
    for wrapper in &wrappers {
        for ty in wrapper
            .parameters
            .iter()
            .chain(std::iter::once(&wrapper.result))
        {
            if let T::Handle(name) = ty {
                handles.insert(name.clone());
            }
        }
    }
    let mut out =
        String::from("// Generated from the KCore service surface.\nimport Foundation\n\n");
    for name in handles {
        let _ = writeln!(out, "public distinct {name} = Int");
    }
    out.push('\n');
    for wrapper in &wrappers {
        let service = manifest
            .services()
            .iter()
            .find(|service| service.symbol == wrapper.symbol)
            .ok_or("service disappeared")?;
        let params = wrapper
            .parameters
            .iter()
            .enumerate()
            .map(|(index, ty)| format!("arg{index}: {}", spelling(ty)))
            .collect::<Vec<_>>()
            .join(", ");
        let result = if wrapper.result == T::Void {
            String::new()
        } else {
            format!(" -> {}", spelling(&wrapper.result))
        };
        let _ = writeln!(out, "public function {}({params}){result} {{", wrapper.name);
        let mut arguments = Vec::new();
        let mut abi = service.signature.parameters.iter();
        for (index, ty) in wrapper.parameters.iter().enumerate() {
            let argument = format!("arg{index}");
            let native = abi
                .next()
                .ok_or_else(|| format!("{}: missing native parameter", wrapper.name))?;
            match ty {
                T::String => {
                    if *native != ServiceType::Pointer
                        || abi.next()
                            != Some(&ServiceType::Integer {
                                bits: 64,
                                signed: true,
                            })
                    {
                        return Err(format!("{}: invalid string ABI", wrapper.name));
                    }
                    arguments.push(format!("{argument}.data"));
                    arguments.push(format!("{argument}.count"));
                }
                _ => arguments.push(argument_for(ty, *native, &argument)?),
            }
        }
        if abi.next().is_some() {
            return Err(format!("{}: extra native parameters", wrapper.name));
        }
        let call = format!("{}({})", wrapper.symbol, arguments.join(", "));
        if wrapper.result == T::Void {
            let _ = writeln!(out, "    {call}");
        } else {
            let _ = writeln!(out, "    let value = {call}");
        }
        if wrapper.result != T::Void {
            let result = result_for(&wrapper.result, service.signature.result, "value")?;
            let _ = writeln!(out, "    return {result}");
        }
        out.push_str("}\n\n");
    }
    Ok(out)
}

fn spelling(ty: &T) -> String {
    match ty {
        T::Integer {
            bits: 64,
            signed: true,
        } => "Int".to_owned(),
        T::Integer { bits, signed } => format!("{}{bits}", if *signed { "I" } else { "U" }),
        T::Float { bits: 64 } => "Float".to_owned(),
        T::Float { bits } => format!("F{bits}"),
        T::Bool => "Bool".to_owned(),
        T::Pointer => "RawPtr".to_owned(),
        T::String => "borrow String".to_owned(),
        T::Handle(name) => name.clone(),
        T::Void => "Void".to_owned(),
    }
}

fn argument_for(ty: &T, native: ServiceType, value: &str) -> Result<String, String> {
    match (ty, native) {
        (
            T::Handle(_),
            ServiceType::Integer {
                bits: 64,
                signed: true,
            },
        ) => Ok(format!("{value}.raw")),
        (T::Pointer, ServiceType::Pointer) => Ok(value.to_owned()),
        (
            T::Integer { bits, signed },
            ServiceType::Integer {
                bits: native_bits,
                signed: native_signed,
            },
        ) if *bits == native_bits && *signed == native_signed => Ok(value.to_owned()),
        (
            T::Integer {
                bits: 8,
                signed: false,
            },
            ServiceType::Integer {
                bits: 64,
                signed: true,
            },
        ) => Ok(format!("Int({value})")),
        (
            T::Integer {
                bits: 64,
                signed: true,
            },
            ServiceType::Integer {
                bits: 32,
                signed: true,
            },
        ) => Ok(format!(
            "I32(({value} & 2147483647) - ({value} & 2147483648))"
        )),
        _ => Err(format!("cannot adapt KCore parameter {ty:?} to {native:?}")),
    }
}

fn result_for(ty: &T, native: ServiceType, value: &str) -> Result<String, String> {
    match (ty, native) {
        (
            T::Handle(name),
            ServiceType::Integer {
                bits: 64,
                signed: true,
            },
        ) => Ok(format!("{name}({value})")),
        (T::Pointer, ServiceType::Pointer) => Ok(value.to_owned()),
        (
            T::Integer { bits, signed },
            ServiceType::Integer {
                bits: native_bits,
                signed: native_signed,
            },
        ) if *bits == native_bits && *signed == native_signed => Ok(value.to_owned()),
        (
            T::Integer {
                bits: 8,
                signed: false,
            },
            ServiceType::Integer {
                bits: 64,
                signed: true,
            },
        ) => Ok(format!("U8({value} & 255)")),
        (
            T::Integer {
                bits: 64,
                signed: true,
            },
            ServiceType::Integer {
                bits: 32,
                signed: true,
            },
        ) => Ok(format!("Int({value})")),
        _ => Err(format!("cannot adapt KCore result {ty:?} from {native:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_every_kcore_service_wrapper() {
        let manifest = KernelServiceManifest::parse(KERNEL_SERVICE_MANIFEST).expect("manifest");
        let wrappers = kcore_services(&manifest).expect("surface");
        let bindings = generate().expect("bindings");
        for wrapper in &wrappers {
            assert!(
                bindings.contains(&format!("public function {}(", wrapper.name)),
                "{}",
                wrapper.name
            );
        }
    }
}
