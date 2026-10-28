//! A shader language's analysis (`KLF-EDITOR.md` section 14): the phases
//! over the project's inputs, then, for each unit that validated, the
//! mandatory passes, binding assignment and every backend the language
//! targets, in memory only.
//!
//! Nothing is written. A refusal names its target, so a user sees which
//! target cannot express the shader rather than which one the build picked.

use crate::shader::NativeOuts;
use crate::{FrontendMetadata, NativePhases};
use klf_core::FileId;
use klf_defsvc::drive_shaders;
use klf_diag::Severity;
use klf_ide::{IdeDiagnostic, ShaderOutput};
use klf_services::Session;
use klf_shader::{Module, Target};
use klf_shader_bind::assign;
use klf_shader_driver::pipeline::{self, Compiled, Settings};
use klf_shader_driver::report::{Problem, Reporter};
use klf_shader_passes::{Bounds, IntegerDivision};
use std::collections::BTreeMap;

/// Runs the phases and the shader pipeline over `files`, recording the facts
/// of sections 14.3 and 14.4 in `session` and returning what the pipeline
/// found.
///
/// # Errors
/// Returns the failure of the phases.
pub(crate) fn run(
    metadata: &FrontendMetadata,
    session: &mut Session,
    files: &[FileId],
) -> Result<Found, String> {
    let Some(family) = metadata.shader.clone() else {
        return Ok(Found::default());
    };
    let mut runner = || Ok(NativePhases::new());
    let modules = drive_shaders(session, files, &mut runner).map_err(|error| error.to_string())?;
    let targets = family.targets.clone();
    let held: BTreeMap<String, FileId> = session
        .files()
        .iter()
        .map(|file| (file.name().as_str().to_owned(), file.id()))
        .collect();
    let mut outs = NativeOuts::new();
    let reporter = Reporter {
        lang: &metadata.name,
        keys: &family.keys,
        specs: &metadata.diagnostics,
    };
    let mut problems = Vec::new();
    let mut found = Found::default();
    for (unit, module) in &modules {
        session.record_facts(records(module, &targets, &held));
        let stem = stem_of(session.file(unit.file()).name().as_str());
        let settings = Settings {
            targets: &targets,
            keep_overrides: false,
            overrides: &BTreeMap::new(),
            bounds: Bounds::Unchecked,
            integer_division: IntegerDivision::Defined,
            spirv_minor: 3,
            debug_info: false,
        };
        if let Some(compiled) = pipeline::compile(
            module.clone(),
            &stem,
            &settings,
            &mut outs,
            &reporter,
            &mut problems,
        ) {
            found.outputs.extend(outputs(unit.file(), &stem, &compiled));
        }
    }
    found.diagnostics = problems
        .iter()
        .filter_map(|problem| diagnostic(metadata, problem, &held))
        .collect();
    Ok(found)
}

/// What a shader analysis found.
#[derive(Default)]
pub(crate) struct Found {
    pub diagnostics: Vec<IdeDiagnostic>,
    pub outputs: Vec<ShaderOutput>,
}

/// Every view of one unit's output the shader produced (section 14.5): the
/// source each target wrote, SPIR-V as disassembly, the KSIR text and the
/// reflection.
fn outputs(file: FileId, stem: &str, compiled: &Compiled) -> Vec<ShaderOutput> {
    let mut out = vec![ShaderOutput {
        file,
        target: Target::Ksl.name().to_owned(),
        kind: "ksir".to_owned(),
        name: format!("{stem}.ksir"),
        language_id: "ksir".to_owned(),
        text: compiled.ksir.clone(),
    }];
    for output in &compiled.outputs {
        for (name, bytes) in &output.files {
            let text = match output.target {
                Target::Spirv => klf_shader_spirv::disasm::disassemble(
                    &klf_shader_spirv::from_hex(&String::from_utf8_lossy(bytes)),
                ),
                _ => String::from_utf8_lossy(bytes).into_owned(),
            };
            out.push(ShaderOutput {
                file,
                target: output.target.name().to_owned(),
                kind: if output.target == Target::Spirv {
                    "spirv".to_owned()
                } else {
                    "source".to_owned()
                },
                name: name.clone(),
                language_id: language_id(output.target).to_owned(),
                text,
            });
        }
        out.push(ShaderOutput {
            file,
            target: output.target.name().to_owned(),
            kind: "reflection".to_owned(),
            name: format!("{stem}.{}.kslr", output.target.name()),
            language_id: "kslr".to_owned(),
            text: klf_shader_reflect::encode(&output.reflection),
        });
    }
    out
}

fn language_id(target: Target) -> &'static str {
    match target {
        Target::Msl => "metal",
        Target::Hlsl => "hlsl",
        Target::Glsl => "glsl",
        Target::Wgsl => "wgsl",
        Target::Ksl => "ksl",
        Target::Spirv => "spirv-dis",
    }
}

/// The kernel's own facts for one module: the bindings, the layouts and the
/// options, which the definition cannot compute (`KLF-EDITOR.md` section 14.3).
fn records(
    module: &Module,
    targets: &[Target],
    held: &BTreeMap<String, FileId>,
) -> klf_facts::Facts {
    if !module.shaders.is_empty()
        && (0..module.shaders.len()).any(|index| assign(module, index).is_ok())
    {
        return klf_shader_ide::record(
            module,
            &klf_shader_ide::Settings {
                targets,
                keep_overrides: false,
            },
            &|path: &str| held.get(path).copied(),
        );
    }
    klf_facts::Facts::default()
}

/// One problem as an editor diagnostic. A problem one target found names it,
/// so the message says which target cannot express the shader (section 14.2).
fn diagnostic(
    metadata: &FrontendMetadata,
    problem: &Problem,
    held: &BTreeMap<String, FileId>,
) -> Option<IdeDiagnostic> {
    let file = problem
        .span
        .as_ref()
        .or(problem.at.as_ref())
        .and_then(|(path, _, _)| held.get(path))
        .copied()?;
    let (_, start, end) = problem
        .span
        .clone()
        .unwrap_or_else(|| (String::new(), 0, 0));
    Some(IdeDiagnostic {
        file,
        start,
        end,
        severity: if problem.warning {
            Severity::Warning
        } else {
            Severity::Error
        },
        code: problem.code.clone(),
        message: problem.message.clone(),
        source: problem
            .target
            .map(|target| format!("{} ({})", metadata.driver, target.name())),
    })
}

fn stem_of(name: &str) -> String {
    std::path::Path::new(name).file_stem().map_or_else(
        || name.to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}
