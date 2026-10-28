//! Steps 4 to 7 of the shader driver loop (section 4.4) for one module:
//! validate, then per target the passes, the backend and reflection.

use crate::Outs;
use crate::report::{Problem, Reporter};
use klf_shader::{Module, Target};
use klf_shader_bind::{Assignment, Resource};
use klf_shader_out::{UnitView, Written};
use klf_shader_passes::{Bounds, IntegerDivision, PassOptions};
use klf_shader_reflect::{Context, Reflection};
use klf_shader_valid::{ModuleInfo, Options, ValidationError, validate};
use std::collections::BTreeMap;

pub struct Settings<'a> {
    pub targets: &'a [Target],
    pub keep_overrides: bool,
    pub overrides: &'a BTreeMap<String, String>,
    pub bounds: Bounds,
    pub integer_division: IntegerDivision,
    pub spirv_minor: u8,
    pub debug_info: bool,
}

/// What one target produced for one shader.
pub struct TargetOutput {
    pub target: Target,
    /// Each output file's name and content, per section 24.2.
    pub files: Vec<(String, Vec<u8>)>,
    pub reflection: Reflection,
}

/// What one module produced.
pub struct Compiled {
    /// The module after lowering and normalization, as textual KSIR.
    pub ksir: String,
    /// Nothing when the module declares no shader: a library of functions.
    pub outputs: Vec<TargetOutput>,
}

/// Compiles `module` for every requested target. Problems come back with
/// warnings among them; any error means nothing is produced.
pub fn compile(
    mut module: Module,
    stem: &str,
    settings: &Settings<'_>,
    outs: &mut dyn Outs,
    reporter: &Reporter<'_>,
    problems: &mut Vec<Problem>,
) -> Option<Compiled> {
    klf_shader_passes::normalize(&mut module);
    // A language declares what its modules may use; each module keeps only
    // what it does use, which is what its outputs then require.
    let used = klf_shader_valid::used_capabilities(&module);
    module
        .capabilities
        .retain(|capability| used.contains(capability));
    let wgsl = settings.targets.contains(&Target::Wgsl);
    let validation = validate(&module, Options { wgsl });
    if !report(&module, &validation.problems, None, reporter, problems) {
        return None;
    }
    let info = validation.info?;
    let ksir = klf_shader::print::print(&module);
    let mut outputs = Vec::new();
    for (index, _) in module.shaders.iter().enumerate() {
        let assignment = match klf_shader_bind::assign(&module, index) {
            Ok(assignment) => assignment,
            Err(errors) => {
                report(&module, &errors, None, reporter, problems);
                return None;
            }
        };
        for target in settings.targets {
            let output = target_output(
                &module,
                &info,
                index,
                &assignment,
                *target,
                stem,
                settings,
                outs,
                reporter,
                problems,
            )?;
            outputs.push(output);
        }
    }
    Some(Compiled { ksir, outputs })
}

/// Reports `errors` and says whether they were only warnings. Every problem
/// one target found is attributed to it (`KLF-EDITOR.md` section 14.2).
fn report(
    module: &Module,
    errors: &[ValidationError],
    target: Option<Target>,
    reporter: &Reporter<'_>,
    problems: &mut Vec<Problem>,
) -> bool {
    let mut clean = true;
    for error in errors {
        let mut problem = reporter.validation(module, error);
        clean &= problem.warning;
        if let Some(target) = target {
            problem = problem.for_target(target);
        }
        problems.push(problem);
    }
    clean
}

#[allow(clippy::too_many_arguments)]
fn target_output(
    module: &Module,
    info: &ModuleInfo,
    shader: usize,
    assignment: &Assignment,
    target: Target,
    stem: &str,
    settings: &Settings<'_>,
    outs: &mut dyn Outs,
    reporter: &Reporter<'_>,
    problems: &mut Vec<Problem>,
) -> Option<TargetOutput> {
    if target == Target::Spirv && settings.spirv_minor < 3 {
        problems.push(Problem::driver(
            "KLF-SHD-0003",
            format!(
                "SPIR-V 1.{} has no StorageBuffer storage class; the backend needs 1.3 or later",
                settings.spirv_minor
            ),
        ));
        return None;
    }
    let options = PassOptions {
        overrides: settings.overrides.clone(),
        keep_overrides: settings.keep_overrides,
        bounds: settings.bounds,
        integer_division: settings.integer_division,
    };
    let (units, baked) = match klf_shader_passes::run(module, target, &options) {
        Ok(result) => result,
        Err(errors) => {
            report(module, &errors, Some(target), reporter, problems);
            return None;
        }
    };
    let wgsl = settings.targets.contains(&Target::Wgsl);
    let mut files = Vec::new();
    for unit in &units {
        let validation = validate(&unit.module, Options { wgsl });
        if !report(
            &unit.module,
            &validation.problems,
            Some(target),
            reporter,
            problems,
        ) {
            return None;
        }
        let unit_info = validation.info?;
        let resources = assignment.for_unit(module, &unit.module);
        let bytes = if target != Target::Spirv && outs.has(target) {
            let view = UnitView::new(
                unit.module.clone(),
                &unit_info,
                &resources,
                target,
                settings.debug_info,
            );
            emitted(&unit.module, outs.emit(view), target, reporter, problems)?
        } else {
            match backend(&unit.module, &unit_info, &resources, target, settings) {
                Ok(bytes) => bytes,
                Err(errors) => {
                    report(&unit.module, &errors, Some(target), reporter, problems);
                    return None;
                }
            }
        };
        let name = match unit.entry {
            None => format!("{stem}.{}", target.extension()),
            Some(entry) => format!(
                "{stem}.{}.{}",
                module.entry_points[entry].stage.suffix(),
                target.extension()
            ),
        };
        files.push((name, bytes));
    }
    let reflection = klf_shader_reflect::reflect(&Context {
        module,
        info,
        shader,
        assignment,
        target,
        baked: &baked,
        keep_overrides: settings.keep_overrides,
    });
    Some(TargetOutput {
        target,
        files,
        reflection,
    })
}

/// What an out wrote, or nothing after reporting why.
fn emitted(
    module: &Module,
    written: Result<Written, String>,
    target: Target,
    reporter: &Reporter<'_>,
    problems: &mut Vec<Problem>,
) -> Option<Vec<u8>> {
    match written {
        Ok(Written::Output(text)) => Some(text.into_bytes()),
        Ok(Written::Refused(errors)) => {
            report(module, &errors, Some(target), reporter, problems);
            None
        }
        Ok(Written::Misused(message)) => {
            problems.push(Problem::driver("KLF-SHD-9001", message));
            None
        }
        Err(message) => {
            problems.push(Problem::driver("KLF-SHD-0004", message));
            None
        }
    }
}

/// The Rust backend of `target`: SPIR-V, which is binary and stays in Rust.
/// Every text target is written by its out (section 26.1).
fn backend(
    module: &Module,
    info: &ModuleInfo,
    resources: &[Option<Resource>],
    target: Target,
    settings: &Settings<'_>,
) -> Result<Vec<u8>, Vec<ValidationError>> {
    if target == Target::Spirv {
        return klf_shader_spirv::write(module, info, resources, settings.debug_info).map(
            |mut words| {
                words[1] = 0x0001_0000 | (u32::from(settings.spirv_minor) << 8);
                klf_shader_spirv::hex(&words).into_bytes()
            },
        );
    }
    Err(vec![ValidationError::keyed(
        "B3",
        "shader.target-unsupported",
        format!("{}: no out writes it in this toolchain", target.name()),
        klf_shader::Span::default(),
    )])
}
