//! A shader language's frontend: the shader driver loop of `KLF-KSL.md`
//! section 4.4 over this binary's compiled phases.

use crate::{EMIT_SELECTOR, NativePhases, bundle, klffe_phase};
use klf_defsvc::{DriveError, FrontendMetadata, ServiceSink, drive_shaders, register_reader};
use klf_native_rt::services;
use klf_shader::Target;
use klf_shader_driver::{Outs, Toolchain};
use klf_shader_out::{UnitView, Written};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The outs this binary holds: the `emit` of each of its definitions,
/// which writes the target the definition is named for (section 5.8). The
/// first definition naming a target is its out.
pub(crate) struct NativeOuts {
    outs: Vec<(Target, i64)>,
}

impl NativeOuts {
    pub(crate) fn new() -> Self {
        let mut outs = Vec::new();
        for (index, language) in bundle::languages().iter().enumerate() {
            let target = match language.name.as_str() {
                "glsl" => Some(Target::Glsl),
                name => Target::from_name(name),
            };
            if let Some(target) = target.filter(|_| language.declares(klf_defsvc::Hook::Emit))
                && !outs.iter().any(|(known, _)| *known == target)
            {
                outs.push((target, index as i64));
            }
        }
        Self { outs }
    }
}

impl Outs for NativeOuts {
    fn has(&self, target: Target) -> bool {
        self.outs.iter().any(|(known, _)| *known == target)
    }

    fn emit(&mut self, view: UnitView) -> Result<Written, String> {
        let target = view.target();
        let index = self
            .outs
            .iter()
            .find(|(known, _)| *known == target)
            .map(|(_, index)| *index)
            .ok_or_else(|| format!("no out writes `{}`", target.name()))?;
        let view = Arc::new(Mutex::new(view));
        let mut sink = ServiceSink::new();
        register_reader(&mut sink, &view);
        services::install(sink.into_table(), String::new());
        let previous = bundle::select(index);
        // SAFETY: `klffe_phase` is emitted into this binary with this
        // signature; selector 6 takes the unit word and ignores the rest.
        let word = unsafe { klffe_phase(EMIT_SELECTOR, 0, 0, 0) };
        bundle::select(previous);
        services::uninstall();
        if word == -1 {
            return Err("the definition has no `emit`".to_owned());
        }
        let view = Arc::try_unwrap(view)
            .map_err(|_| "the out's services outlived it".to_owned())?
            .into_inner()
            .map_err(|_| "a service of the out panicked".to_owned())?;
        Ok(view.finish(word == 1))
    }
}

/// The library shipped beside the driver in a sysroot
/// (`share/<lang>/library`, section 5.6), or the package's own when the
/// frontend runs from where it was built.
fn library(metadata: &FrontendMetadata) -> Option<PathBuf> {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent()?.parent().map(PathBuf::from))
        .map(|root| root.join("share").join(&metadata.name).join("library"));
    if let Some(installed) = installed.filter(|path| path.is_dir()) {
        return Some(installed);
    }
    (!metadata.std_root.is_empty()).then(|| PathBuf::from(&metadata.std_root))
}

pub(crate) fn run(metadata: &FrontendMetadata, arguments: &[String]) -> i32 {
    let Some(family) = metadata.shader.clone() else {
        return 1;
    };
    // A shader driver is the same generated CLI: same surface, same phases.
    let _display = klf_diag::status::begin(&format!("Compiling {}", metadata.driver), false);
    let toolchain = Toolchain {
        lang: metadata.name.clone(),
        driver: metadata.driver.clone(),
        version: metadata.version.clone(),
        extensions: metadata.extensions.clone(),
        family,
        diagnostics: metadata.diagnostics.clone(),
        library: library(metadata),
        triple: metadata.target.clone(),
    };
    let name = metadata.name.clone();
    let mut lower = |session: &mut klf_services::Session, files: &[klf_core::FileId]| {
        let mut make_runner = || Ok(NativePhases::new());
        drive_shaders(session, files, &mut make_runner).map_err(|error| match error {
            DriveError::Phase { message, .. } => message,
            other => format!("{name}: {other}"),
        })
    };
    let mut outs = NativeOuts::new();
    klf_shader_driver::run(&toolchain, arguments, &mut lower, &mut outs)
}
