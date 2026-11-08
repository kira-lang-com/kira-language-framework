//! RIDL exports implemented by compiled Kira VM code.

use crate::runtime::{Host, Service};
use crate::vm::{decode_scalar, encode_scalar, scalar_width};
use klf_kir::{Module as KirModule, Type as KirType};
use klf_ridl::meta::Metadata;
use klf_ridl::module::{Convention, ItemKind};
use klf_ridl::types::{Scalar, Type};
use klf_ridl_wire::{Reply, Value};
use klf_vm::{Capabilities, Instance, Outcome};
use std::collections::{HashMap, HashSet};
use std::io::{self, ErrorKind};
use std::path::Path;
use std::sync::Arc;

fn invalid(reason: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, reason)
}

fn attribute<'a>(module: &'a KirModule, name: &str) -> impl Iterator<Item = &'a str> {
    module
        .attributes
        .iter()
        .filter(move |attribute| attribute.name == name)
        .filter_map(|attribute| attribute.value.as_deref())
}

impl Host {
    /// Loads a compiled Kira host service without per-method Rust callbacks.
    /// The VM image, KIR-generated RIDL records and ABI metadata must all be
    /// built from the same module. Its foreign services are explicitly granted
    /// by the embedder, rather than falling back to the native symbol table.
    ///
    /// # Errors
    /// Returns an error if a compiled artifact cannot be read, validated,
    /// initialized or registered as a RIDL provider.
    pub fn export_compiled_vm(
        &mut self,
        image: impl AsRef<Path>,
        kir: impl AsRef<Path>,
        metadata: impl AsRef<Path>,
        capabilities: Capabilities,
    ) -> io::Result<Arc<Instance>> {
        let program = klf_vm::decode(&std::fs::read(image)?)
            .map_err(|error| invalid(&format!("invalid compiled Kira host bytecode: {error}")))?;
        let module = klf_kir::kirb::decode(&std::fs::read(kir)?)
            .ok_or_else(|| invalid("invalid compiled Kira host import/export map"))?;
        let metadata = Metadata::decode(&std::fs::read(metadata)?)
            .map_err(|error| invalid(&format!("invalid Kira host RIDL metadata: {error}")))?;
        let instance: Arc<Instance> = Arc::from(
            Instance::load_restricted(program, capabilities)
                .map_err(|error| invalid(&format!("invalid Kira host VM imports: {error}")))?,
        );
        if !matches!(instance.initialize(), Outcome::Return(_)) {
            return Err(invalid("compiled Kira host failed to initialize"));
        }
        self.export_vm(&module, Arc::clone(&instance), metadata)?;
        Ok(instance)
    }

    /// Registers a Kira VM's generated RIDL exports as host capabilities.
    /// The module's `ridl.surface` and `ridl.entry` are the compiler's own
    /// declarations; no Rust callback is written for each exported function.
    ///
    /// The exporting instance must have completed initialization and must not
    /// execute a blocking `Plugin.wait` on that same VM. RIDL dispatch runs on
    /// the host worker; an actively executing VM cannot be entered by another
    /// thread until its current call returns.
    ///
    /// # Errors
    /// Rejects missing or inconsistent RIDL metadata, unsupported layouts,
    /// and exports that cannot be found in the compiled VM image.
    pub fn export_vm(
        &mut self,
        module: &KirModule,
        instance: Arc<Instance>,
        metadata: Metadata,
    ) -> io::Result<()> {
        let name = attribute(module, "ridl.module")
            .next()
            .ok_or_else(|| invalid("Kira host has no generated RIDL module"))?;
        let surface = attribute(module, "ridl.surface")
            .next()
            .ok_or_else(|| invalid("Kira host has no generated RIDL surface"))?;
        let interface = klf_ridl::parse(klf_core::FileId::from_u32(0), name, surface)
            .map_err(|error| invalid(&format!("invalid Kira host RIDL surface: {error}")))?;
        if metadata.module_name != name {
            return Err(invalid("Kira host RIDL metadata names a different module"));
        }
        let digest = Metadata::for_module(&interface, Vec::new(), Vec::new(), Vec::new()).sha256;
        if metadata.sha256 != digest {
            return Err(invalid(
                "Kira host RIDL metadata does not match its generated surface",
            ));
        }

        let mut declared = HashMap::new();
        for item in &interface.items {
            if let ItemKind::Function(function) = &item.kind {
                declared.insert(function.name.as_str(), function);
            }
        }
        let mut functions: HashMap<String, Service> = HashMap::new();
        let mut seen = HashSet::new();
        for entry in attribute(module, "ridl.entry") {
            let mut fields = entry.split('\t');
            let (Some(path), Some(symbol)) = (fields.next(), fields.next()) else {
                return Err(invalid("malformed generated Kira RIDL entry"));
            };
            let Some(function) = declared.get(path) else {
                return Err(invalid("Kira RIDL entry is absent from its surface"));
            };
            if !seen.insert(path) {
                return Err(invalid("duplicate generated Kira RIDL entry"));
            }
            if !function.generics.is_empty() || function.self_param.is_some() {
                return Err(invalid(
                    "remote Kira RIDL method needs a non-scalar adapter",
                ));
            }
            let parameters: Vec<Scalar> = function
                .params
                .iter()
                .map(|param| match (&param.ty, param.convention) {
                    (Type::Scalar(ty), Convention::Owned) if scalar_width(*ty).is_some() => Ok(*ty),
                    _ => Err(invalid(
                        "remote Kira RIDL parameter needs a non-scalar adapter",
                    )),
                })
                .collect::<io::Result<_>>()?;
            let result = match function.result.as_ref() {
                None | Some(Type::Scalar(Scalar::Void)) => None,
                Some(Type::Scalar(ty)) if scalar_width(*ty).is_some() => Some(*ty),
                _ => {
                    return Err(invalid(
                        "remote Kira RIDL result needs a non-scalar adapter",
                    ));
                }
            };
            let Some(index) = instance.function_index(symbol) else {
                return Err(invalid("generated Kira RIDL entry has no VM function"));
            };
            let Some(compiled) = module
                .functions
                .iter()
                .find(|entry| entry.name.as_str() == symbol)
            else {
                return Err(invalid(
                    "generated Kira RIDL entry is absent from its KIR module",
                ));
            };
            if compiled.foreign
                || compiled.failure.is_some()
                || compiled.parameters.len() != parameters.len()
                || compiled
                    .parameters
                    .iter()
                    .zip(&parameters)
                    .any(|(parameter, scalar)| !matches_scalar(&parameter.ty, *scalar))
                || match result {
                    Some(scalar) => !matches_scalar(&compiled.result, scalar),
                    None => compiled.result != KirType::Unit,
                }
            {
                return Err(invalid(
                    "generated Kira RIDL entry disagrees with its KIR signature",
                ));
            }
            if instance.parameter_count(index) != parameters.len() {
                return Err(invalid(
                    "generated Kira RIDL entry has a different parameter count",
                ));
            }
            let vm = Arc::clone(&instance);
            let function: Service = Arc::new(move |arguments| {
                if arguments.len() != parameters.len() {
                    return Reply::Refused(
                        "RIDL argument count does not match the contract".into(),
                    );
                }
                let Some(words) = arguments
                    .iter()
                    .cloned()
                    .zip(&parameters)
                    .map(|(value, ty)| decode_scalar(*ty, value))
                    .collect::<Option<Vec<_>>>()
                else {
                    return Reply::Refused(
                        "RIDL argument type or width does not match the contract".into(),
                    );
                };
                let Some(outcome) = vm.try_call(index, &words) else {
                    return Reply::Refused("Kira host VM is executing on another thread".into());
                };
                match (outcome, result) {
                    (Outcome::Return(Some(word)), Some(ty)) => {
                        Reply::Returned(encode_scalar(ty, word))
                    }
                    (Outcome::Return(None), None) => Reply::Returned(Value::Void),
                    (Outcome::Trap(reason), _) => {
                        Reply::Refused(format!("Kira RIDL host trapped: {reason}"))
                    }
                    (Outcome::Fail(_), _) => {
                        Reply::Refused("Kira RIDL host function failed".into())
                    }
                    _ => Reply::Refused("Kira RIDL host returned the wrong value shape".into()),
                }
            });
            let callbacks = Arc::clone(&self.callbacks);
            functions.insert(
                path.to_owned(),
                Arc::new(move |arguments| {
                    let function = Arc::clone(&function);
                    let arguments = arguments.to_vec();
                    callbacks.invoke(Box::new(move || function(&arguments)))
                }),
            );
        }
        if functions.len() != declared.len() {
            return Err(invalid(
                "Kira host RIDL surface has functions without generated entries",
            ));
        }
        self.export(metadata, functions)
    }
}

fn matches_scalar(ty: &KirType, scalar: Scalar) -> bool {
    match (ty, scalar) {
        (KirType::Float(32), Scalar::F32) | (KirType::Float(64), Scalar::F64) => true,
        (KirType::Integer(width), Scalar::I8 | Scalar::U8) => *width == 8,
        (KirType::Integer(width), Scalar::I16 | Scalar::U16) => *width == 16,
        (KirType::Integer(width), Scalar::I32 | Scalar::U32) => *width == 32,
        (KirType::Integer(width), Scalar::I64 | Scalar::U64 | Scalar::ISize | Scalar::USize) => {
            *width == 64
        }
        (KirType::Integer(1), Scalar::Bool) => true,
        _ => false,
    }
}
