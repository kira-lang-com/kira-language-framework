//! The shader driver loop (`KLF-KSL.md` section 4.4): what a shader
//! language's driver runs around its definition's phases. Reads the command
//! line of section 5.7, lowers every input through the definition, then
//! validates, runs the passes and backends, reflects, and writes the files
//! of section 24.2 only when every target succeeded for every input.
//!
//! The definition's phases are reached through [`Lower`], so a natively
//! compiled frontend and the VM-hosted one share this loop.

pub mod arguments;
pub mod pipeline;
pub mod report;

use arguments::{Command, Emit, Invocation};
use klf_core::FileId;
use klf_services::{DefinitionUnit, DiagnosticSpec, Session, ShaderFamily, ShaderSettings};
use klf_shader::{Module, Target};
use klf_shader_out::{UnitView, Written};
use pipeline::{Compiled, Settings};
use report::{Problem, Reporter};
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// What a shader driver knows about its language.
pub struct Toolchain {
    pub lang: String,
    pub driver: String,
    pub version: String,
    pub extensions: Vec<String>,
    pub family: ShaderFamily,
    pub diagnostics: Vec<DiagnosticSpec>,
    /// The shared shader modules shipped with the toolchain, if any.
    pub library: Option<PathBuf>,
    /// The host triple the Session is created for.
    pub triple: String,
}

/// Runs `units`, `parse`, `elaborate` and `lower` over the inputs: one KSIR
/// module per unit, or the definition's rendered diagnostics.
pub type Lower<'a> =
    dyn FnMut(&mut Session, &[FileId]) -> Result<Vec<(DefinitionUnit, Module)>, String> + 'a;

/// The outs a toolchain holds (section 5.8).
pub trait Outs {
    /// Whether an out writes `target`.
    fn has(&self, target: Target) -> bool;

    /// Runs the out for `view`'s target over it.
    ///
    /// # Errors
    /// Returns a message when the out could not be run at all.
    fn emit(&mut self, view: UnitView) -> Result<Written, String>;
}

/// Whether this toolchain can write `target` at all: SPIR-V from the built-in
/// backend, every other target from an out.
fn writes(outs: &dyn Outs, target: Target) -> bool {
    target == Target::Spirv || outs.has(target)
}

/// Runs the driver over `arguments` (after the program name) and returns
/// the process exit status.
pub fn run(
    toolchain: &Toolchain,
    arguments: &[String],
    lower: &mut Lower<'_>,
    outs: &mut dyn Outs,
) -> i32 {
    match arguments.first().map(String::as_str) {
        Some("--help" | "-h") | None => {
            println!("{}", arguments::usage(&toolchain.driver));
            return 0;
        }
        Some("--version") => {
            println!("{} {}", toolchain.driver, toolchain.version);
            return 0;
        }
        _ => {}
    }
    let invocation = match arguments::parse(arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("{}: error: {message}", toolchain.driver);
            return 1;
        }
    };
    match drive(toolchain, &invocation, lower, outs) {
        Ok(()) => 0,
        Err(problems) => {
            for problem in problems {
                eprintln!("{}", problem.render(invocation.json));
            }
            1
        }
    }
}

fn drive(
    toolchain: &Toolchain,
    invocation: &Invocation,
    lower: &mut Lower<'_>,
    outs: &mut dyn Outs,
) -> Result<(), Vec<Problem>> {
    let targets = targets(toolchain, invocation, outs).map_err(|problem| vec![problem])?;
    let inputs = inputs(toolchain, invocation).map_err(|problem| vec![problem])?;
    let mut session = session(toolchain).map_err(|problem| vec![problem])?;
    let mut files = Vec::new();
    for path in &inputs {
        let text = session.read_text(path).map_err(|error| {
            vec![Problem::driver(
                "KLF-SHD-0001",
                format!("cannot read {}: {error}", path.display()),
            )]
        })?;
        let file = session.add_file(path.to_string_lossy().as_ref(), text);
        session.register_unit(DefinitionUnit::new(file));
        files.push(file);
    }
    let reporter = Reporter {
        lang: &toolchain.lang,
        keys: &toolchain.family.keys,
        specs: &toolchain.diagnostics,
    };
    let modules =
        lower(&mut session, &files).map_err(|rendered| reporter.definition(&session, rendered))?;
    let settings = Settings {
        targets: &targets,
        keep_overrides: invocation.keep_overrides,
        overrides: &invocation.overrides,
        bounds: invocation.bounds,
        integer_division: invocation.integer_division,
        spirv_minor: invocation.spirv_minor,
        debug_info: invocation.debug_info,
    };
    let mut problems = Vec::new();
    let mut compiled = Vec::new();
    for (unit, module) in modules {
        let name = session.file(unit.file()).name().as_str().to_owned();
        let stem = Path::new(&name)
            .file_stem()
            .map_or_else(|| name.clone(), |stem| stem.to_string_lossy().into_owned());
        if let Some(result) =
            pipeline::compile(module, &stem, &settings, outs, &reporter, &mut problems)
        {
            compiled.push((stem, result));
        }
    }
    let failed = problems.iter().any(|problem| !problem.warning);
    for problem in problems.iter().filter(|problem| problem.warning) {
        eprintln!("{}", problem.render(invocation.json));
    }
    if failed {
        return Err(problems
            .into_iter()
            .filter(|problem| !problem.warning)
            .collect());
    }
    if invocation.command == Command::Check {
        return Ok(());
    }
    match invocation.emit {
        Some(emit) => {
            print!("{}", emitted(emit, &compiled));
            Ok(())
        }
        None => write(invocation, &compiled).map_err(|problem| vec![problem]),
    }
}

fn targets(
    toolchain: &Toolchain,
    invocation: &Invocation,
    outs: &dyn Outs,
) -> Result<Vec<Target>, Problem> {
    // KSL is written only on request (section 4.5), and by any toolchain,
    // since the `ksl` out is linked with every one (section 5.9).
    //
    // Asked for no target, a toolchain writes every one it can actually write.
    // Naming every target the language's family declares instead would have it
    // build for targets it holds no out for, and report each as one the
    // toolchain cannot express — a shader that compiles answers nothing, which
    // reads as the shader being wrong rather than the toolchain being built
    // without an out. A toolchain that can write none falls back to the
    // family's list, so the failure names a target rather than doing nothing.
    if invocation.targets.is_empty() {
        let writable: Vec<Target> = toolchain
            .family
            .targets
            .iter()
            .copied()
            .filter(|target| *target != Target::Ksl)
            .filter(|target| writes(outs, *target))
            .collect();
        if writable.is_empty() {
            return Ok(toolchain
                .family
                .targets
                .iter()
                .copied()
                .filter(|target| *target != Target::Ksl)
                .collect());
        }
        return Ok(writable);
    }
    for target in &invocation.targets {
        if *target != Target::Ksl && !toolchain.family.targets.contains(target) {
            return Err(Problem::driver(
                "KLF-DRV-0030",
                format!("{} does not target {}", toolchain.lang, target.name()),
            ));
        }
    }
    Ok(invocation.targets.clone())
}

/// The input files: each file given, and every file under a directory with
/// one of the language's extensions. No path means the package's
/// `Shaders/` directory, or the current one without it.
fn inputs(toolchain: &Toolchain, invocation: &Invocation) -> Result<Vec<PathBuf>, Problem> {
    let paths = if invocation.paths.is_empty() {
        let shaders = PathBuf::from("Shaders");
        vec![if shaders.is_dir() {
            shaders
        } else {
            PathBuf::from(".")
        }]
    } else {
        invocation.paths.clone()
    };
    let mut found = Vec::new();
    for path in &paths {
        if path.is_dir() {
            let mut under = Vec::new();
            walk(path, &toolchain.extensions, &mut under)?;
            under.sort();
            found.extend(under);
        } else {
            found.push(path.clone());
        }
    }
    if found.is_empty() {
        return Err(Problem::driver("KLF-SHD-0001", "no shader files"));
    }
    Ok(found)
}

fn walk(directory: &Path, extensions: &[String], found: &mut Vec<PathBuf>) -> Result<(), Problem> {
    let entries = std::fs::read_dir(directory).map_err(|error| {
        Problem::driver(
            "KLF-SHD-0001",
            format!("cannot read {}: {error}", directory.display()),
        )
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        let wanted = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extensions.iter().any(|known| known == extension));
        if path.is_dir() {
            walk(&path, extensions, found)?;
        } else if wanted {
            found.push(path);
        }
    }
    Ok(())
}

fn session(toolchain: &Toolchain) -> Result<Session, Problem> {
    let target = klf_target::Target::from_str(&toolchain.triple).map_err(|error| {
        Problem::driver(
            "KLF-SHD-0001",
            format!("target `{}`: {error:?}", toolchain.triple),
        )
    })?;
    let mut session = Session::new(target);
    session.set_shader(ShaderSettings {
        lang: toolchain.lang.clone(),
        layouts: toolchain.family.layouts,
        capabilities: toolchain.family.capabilities.clone(),
    });
    if let Some(library) = &toolchain.library {
        session.set_std_root(library.to_string_lossy().into_owned());
    }
    session.register_diagnostics(toolchain.diagnostics.clone());
    Ok(session)
}

fn emitted(emit: Emit, compiled: &[(String, Compiled)]) -> String {
    let mut out = String::new();
    for (_, result) in compiled {
        if emit == Emit::Ksir {
            out.push_str(&result.ksir);
            continue;
        }
        for output in &result.outputs {
            match emit {
                Emit::Source => {
                    for (_, bytes) in &output.files {
                        out.push_str(&String::from_utf8_lossy(bytes));
                        if !out.ends_with('\n') {
                            out.push('\n');
                        }
                    }
                }
                Emit::Reflection => out.push_str(&klf_shader_reflect::encode(&output.reflection)),
                Emit::Digest => {
                    out.push_str(&klf_shader_reflect::resource_digest(&output.reflection));
                    out.push('\n');
                }
                Emit::Ksir => {}
            }
        }
    }
    out
}

/// Writes every output under `--out` (default `generated/shaders`), with one
/// `.resources` digest per shader that has resources.
fn write(invocation: &Invocation, compiled: &[(String, Compiled)]) -> Result<(), Problem> {
    let directory = invocation
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("generated/shaders"));
    let fail = |path: &Path, error: std::io::Error| {
        Problem::driver(
            "KLF-SHD-0002",
            format!("cannot write {}: {error}", path.display()),
        )
    };
    std::fs::create_dir_all(&directory).map_err(|error| fail(&directory, error))?;
    for (stem, result) in compiled {
        for output in &result.outputs {
            for (name, bytes) in &output.files {
                let path = directory.join(name);
                std::fs::write(&path, bytes).map_err(|error| fail(&path, error))?;
            }
        }
        // A shader without resources has no digest file, as in the oracle.
        if let Some(first) = result.outputs.first() {
            let digest = klf_shader_reflect::resource_digest(&first.reflection);
            if !digest.is_empty() {
                let path = directory.join(format!("{stem}.resources"));
                std::fs::write(&path, digest).map_err(|error| fail(&path, error))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Outs, Target, Toolchain, UnitView, Written, targets};
    use crate::Invocation;
    use klf_services::ShaderFamily;
    use klf_shader::builder::LayoutDefaults;
    use klf_shader_passes::{Bounds, IntegerDivision};
    use std::collections::BTreeMap;

    /// An out for `names`, and nothing for the targets it does not name.
    struct Held(Vec<&'static str>);

    impl Outs for Held {
        fn has(&self, target: Target) -> bool {
            self.0.contains(&target.name())
        }

        fn emit(&mut self, _view: UnitView) -> Result<Written, String> {
            Err("an out this test never runs".to_owned())
        }
    }

    fn toolchain() -> Toolchain {
        Toolchain {
            lang: "msl".to_owned(),
            driver: "kmsl".to_owned(),
            version: "0.1.0".to_owned(),
            extensions: Vec::new(),
            family: ShaderFamily {
                layouts: LayoutDefaults {
                    uniform: klf_shader::LayoutRule::Std140,
                    storage: klf_shader::LayoutRule::Std430,
                    immediate: klf_shader::LayoutRule::Scalar,
                },
                targets: Target::ALL.to_vec(),
                library_root: String::new(),
                keys: Vec::new(),
                capabilities: Vec::new(),
            },
            diagnostics: Vec::new(),
            library: None,
            triple: "x86_64-unknown-linux-gnu".to_owned(),
        }
    }

    fn invocation(targets: &[&str]) -> Invocation {
        Invocation {
            command: crate::Command::Build,
            paths: Vec::new(),
            targets: targets
                .iter()
                .map(|name| Target::from_name(name).expect("a known target"))
                .collect(),
            out: None,
            emit: None,
            keep_overrides: false,
            overrides: BTreeMap::new(),
            bounds: Bounds::Unchecked,
            integer_division: IntegerDivision::Defined,
            spirv_minor: 3,
            json: false,
            debug_info: false,
        }
    }

    /// A toolchain asked for no target writes what it can write. The language
    /// declares every target its family supports, and a driver holds one out
    /// per target it was built with, so defaulting to the family instead would
    /// build for targets with no out and report each as one the toolchain
    /// cannot express — a shader that compiles answering nothing.
    #[test]
    fn asked_for_no_target_a_toolchain_writes_only_what_it_can() {
        let got = targets(&toolchain(), &invocation(&[]), &Held(vec!["msl"]))
            .expect("the default resolves");
        assert_eq!(got, vec![Target::Msl, Target::Spirv]);
    }

    /// SPIR-V needs no out: the built-in backend writes it, so it is always
    /// among the defaults.
    #[test]
    fn spirv_needs_no_out_to_be_writable() {
        let got =
            targets(&toolchain(), &invocation(&[]), &Held(vec![])).expect("the default resolves");
        assert_eq!(got, vec![Target::Spirv]);
    }

    /// KSL is written only when asked for (section 4.5), so it is never a
    /// default even from a toolchain holding that out.
    #[test]
    fn ksl_is_never_a_default() {
        let got = targets(&toolchain(), &invocation(&[]), &Held(vec!["ksl"])).expect("resolves");
        assert!(!got.contains(&Target::Ksl), "{got:?}");
    }

    /// A toolchain that can write nothing falls back to the family's list, so
    /// the failure names a target instead of silently emitting nothing.
    #[test]
    fn a_toolchain_that_can_write_nothing_still_names_its_targets() {
        // SPIR-V is always writable, so this needs a family without it to be
        // a real fallback: nothing at all to write, and the family's list.
        let family = Toolchain {
            family: ShaderFamily {
                targets: vec![Target::Hlsl, Target::Wgsl],
                ..toolchain().family
            },
            ..toolchain()
        };
        let got = targets(&family, &invocation(&[]), &Held(vec![])).expect("resolves");
        assert_eq!(got, vec![Target::Hlsl, Target::Wgsl]);
    }

    /// Asked for a target, the toolchain writes it, and one it does not hold
    /// is refused by name rather than quietly dropped.
    #[test]
    fn an_asked_for_target_is_written_whether_or_not_it_can() {
        let outs = Held(vec!["msl"]);
        let got = targets(&toolchain(), &invocation(&["msl"]), &outs).expect("resolves");
        assert_eq!(got, vec![Target::Msl]);
    }

    #[test]
    fn a_target_outside_the_family_is_refused_by_name() {
        let family = Toolchain {
            family: ShaderFamily {
                targets: vec![Target::Msl],
                ..toolchain().family
            },
            ..toolchain()
        };
        let error = targets(&family, &invocation(&["wgsl"]), &Held(vec!["wgsl"]))
            .expect_err("wgsl is not in this family");
        assert!(
            error.message.contains("wgsl"),
            "the message names the target: {}",
            error.message
        );
    }
}
