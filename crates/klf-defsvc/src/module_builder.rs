use crate::default_policy;
use crate::{DefinitionArtifactKind, DefinitionMaterialization, DefinitionServiceState};
use klf_kir::Module;
use klf_services::{DefaultConvention, Exclusivity, MemoryModel};

impl DefinitionServiceState {
    pub(super) fn module_finish(&mut self, arguments: &[u64]) -> u64 {
        let Some(module) = arguments.first().copied() else {
            return u64::MAX;
        };
        let Some((id, DefinitionArtifactKind::Module)) = self.output else {
            return u64::MAX;
        };
        let result = self.finish_builder(module).and_then(|mut finished| {
            crate::evaluation::fold_calls(
                &mut finished,
                &self.comptime_functions,
                &self.comptime_buffers,
            )?;
            Ok(finished)
        });
        match result {
            Ok(finished) => {
                self.materialized = Some(DefinitionMaterialization::Module(Box::new(finished)));
            }
            Err(error) => self.module_error = Some(error),
        }
        u64::try_from(id.index()).unwrap_or(u64::MAX)
    }

    pub(super) fn finish_builder(&mut self, module: u64) -> Result<Module, String> {
        let module_index = usize::try_from(module).map_err(|_| "invalid KIR module handle")?;
        let mut types = std::mem::take(&mut self.types).into_vec();
        if types.iter().any(|(owner, _)| *owner != module) {
            return Err("KIR builder contains types owned by another module".to_owned());
        }
        let memory = self
            .file
            .and_then(|file| self.file_memories.get(file.index()).copied().flatten())
            .unwrap_or(MemoryModel::Counted);
        let exclusivity = self
            .file
            .and_then(|file| self.file_exclusivity(file))
            .map_or(
                klf_kir::Exclusivity::None,
                |exclusivity| match exclusivity {
                    Exclusivity::Static => klf_kir::Exclusivity::Static,
                    Exclusivity::Dynamic => klf_kir::Exclusivity::Dynamic,
                    Exclusivity::None => klf_kir::Exclusivity::None,
                },
            );
        let lifecycle_functions =
            match default_policy::derive_all(&types, module, memory, exclusivity) {
                Ok((policies, functions)) => {
                    for ((_, declaration), policy) in types.iter_mut().zip(policies) {
                        declaration.policy = policy;
                    }
                    functions
                }
                Err(error) => {
                    return Err(error);
                }
            };
        let Some(mut module_builder) = self.modules.get_mut(module_index).and_then(Option::take)
        else {
            return Err("unknown KIR module builder".to_owned());
        };
        for (_, declaration) in types {
            module_builder.add_type(declaration);
        }
        for function in lifecycle_functions {
            module_builder.add_function(function);
        }
        for slot in &mut self.kir_globals {
            let belongs = slot.as_ref().is_some_and(|global| global.module == module);
            if belongs {
                let global = slot.take().expect("pending global exists");
                module_builder.add_global(klf_kir::GlobalDeclaration {
                    name: global.name,
                    linkage: klf_kir::Linkage::Module,
                    ty: global.ty,
                    init: global.init,
                    mutable: global.mutable,
                    thread_local: global.thread_local,
                });
            }
        }
        let function_abi = self
            .file
            .and_then(|file| self.file_conventions.get(file.index()).copied().flatten())
            .map_or(
                klf_kir::FunctionAbi::Platform,
                |convention| match convention {
                    DefaultConvention::Ridl => klf_kir::FunctionAbi::Ridl,
                    DefaultConvention::Platform => klf_kir::FunctionAbi::Platform,
                },
            );
        for slot in &mut self.functions {
            let belongs = slot
                .as_ref()
                .is_some_and(|function| function.module == module);
            if belongs {
                let function = slot.take().expect("builder function exists");
                let mut function = function.builder.finish();
                // C enters a function with a C prototype, so it keeps the
                // platform convention.
                if !function.foreign && function.c_prototype.is_none() {
                    function.abi = function_abi;
                }
                module_builder.add_function(function);
            }
        }
        Ok(module_builder.finish())
    }
}
