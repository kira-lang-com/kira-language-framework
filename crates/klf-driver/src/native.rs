//! Native builds and execution: lower KIR with Cranelift and link it against
//! the shared `klf-native-rt` runtime with the provisioned clang.

use klf_diag::progress;
use klf_kir::Module;
use klf_vm::{Outcome, RuntimeDiagnostic};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Why native compilation or execution could not be performed.
#[derive(Debug)]
pub enum NativeError {
    /// The provisioned toolchain is not installed on this machine.
    Unavailable(String),
    /// Verification, lowering, linking, or execution failed.
    Failed(String),
}

impl std::fmt::Display for NativeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(message) => {
                write!(formatter, "native toolchain unavailable: {message}")
            }
            Self::Failed(message) => write!(formatter, "native run failed: {message}"),
        }
    }
}

/// Returns whether the provisioned clang needed for native builds is present.
#[must_use]
pub fn toolchain_available() -> bool {
    find_clang().is_some()
}

/// Builds a host-native executable for one KIR entry function.
///
/// # Errors
/// Returns an error when the native toolchain, KIR verification, Cranelift
/// lowering, or linking fails.
pub fn build_native(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    output: &Path,
    profile: crate::Profile,
) -> Result<(), String> {
    let mut module = host_module(module);
    klf_kir_passes::optimize(
        &mut module,
        profile.level(),
        klf_kir_passes::Backend::Cranelift,
    );
    link_module(&module, entry, arguments, output).map_err(|error| match error {
        NativeError::Unavailable(message) => {
            format!("native toolchain unavailable: {message}")
        }
        NativeError::Failed(message) => format!("native build failed: {message}"),
    })
}

/// Builds and runs one native entry, collecting runtime diagnostics.
///
/// # Errors
/// Returns an error when the toolchain, verification, lowering, linking, or
/// execution fails.
pub fn run_native_with_diagnostics(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    source: Option<&str>,
) -> Result<(Outcome, Vec<RuntimeDiagnostic>), NativeError> {
    let module = host_module(module);
    let bin = module_temp_dir(&module, entry, arguments).join("program.out");
    link_module(&module, entry, arguments, &bin)?;

    let mut run = Command::new(&bin);
    if let Some(source) = source {
        run.env("KLF_SOURCE", source);
    }
    let output = run
        .output()
        .map_err(|e| NativeError::Failed(format!("run: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_output(&stdout)
}

fn link_module(
    module: &Module,
    entry: &str,
    arguments: &[u64],
    output: &Path,
) -> Result<(), NativeError> {
    progress!("verify KIR");
    let diagnostics = klf_kir_verify::verify_module(module);
    if !diagnostics.is_empty() {
        let diagnostics = diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        return Err(NativeError::Failed(format!(
            "KIR verification failed:\n{diagnostics}"
        )));
    }
    progress!("codegen {entry}");
    let object = klf_cranelift::emit_object(module, entry, arguments)
        .map_err(|e| NativeError::Failed(format!("cranelift: {e:?}")))?;
    progress!("build runtime");
    let runtime = build_runtime().map_err(NativeError::Failed)?;

    let dir = module_temp_dir(module, entry, arguments);
    std::fs::create_dir_all(&dir).map_err(|e| NativeError::Failed(e.to_string()))?;
    let object_path = dir.join("program.o");
    std::fs::write(&object_path, object).map_err(|e| NativeError::Failed(e.to_string()))?;

    progress!("link {}", output.display());
    crate::kernel::link(
        &[object_path],
        &[runtime],
        output,
        crate::kernel::host_triple(),
    )
    .map_err(NativeError::Failed)
}

fn module_temp_dir(module: &Module, entry: &str, arguments: &[u64]) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    module.to_string().hash(&mut hasher);
    let source_hash = hasher.finish();
    std::env::temp_dir().join(format!(
        "klf-driver-{entry}-{}-{source_hash:016x}",
        arguments.len()
    ))
}

fn parse_outcome(stdout: &str) -> Result<Outcome, NativeError> {
    let line = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();
    if let Some(code) = line.strip_prefix("TRAP ") {
        return Ok(Outcome::Trap(code.to_owned()));
    }
    if let Some(value) = line.strip_prefix("FAIL ") {
        return value
            .trim()
            .parse()
            .map(Outcome::Fail)
            .map_err(|_| NativeError::Failed(format!("bad FAIL value: {line}")));
    }
    if let Some(rest) = line.strip_prefix("RET ") {
        let rest = rest.trim();
        if rest == "void" {
            return Ok(Outcome::Return(None));
        }
        return rest
            .parse()
            .map(|v| Outcome::Return(Some(v)))
            .map_err(|_| NativeError::Failed(format!("bad RET value: {line}")));
    }
    Err(NativeError::Failed(format!(
        "unrecognized native output: {stdout:?}"
    )))
}

fn parse_output(stdout: &str) -> Result<(Outcome, Vec<RuntimeDiagnostic>), NativeError> {
    let mut diagnostics: Vec<RuntimeDiagnostic> = Vec::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("DIAGD ") {
            let detail = klf_diag::RuntimeDetail::from_protocol(rest)
                .ok_or_else(|| NativeError::Failed(format!("bad DIAGD value: {line}")))?;
            if let Some(last) = diagnostics.last_mut() {
                last.details.push(detail);
            }
            continue;
        }
        let Some((with_subject, rest)) = line
            .strip_prefix("DIAGS ")
            .map(|rest| (true, rest))
            .or_else(|| line.strip_prefix("DIAG ").map(|rest| (false, rest)))
        else {
            continue;
        };
        let values: Vec<_> = rest.split_whitespace().collect();
        if values.len() != if with_subject { 7 } else { 5 } {
            return Err(NativeError::Failed(format!("bad DIAG value: {rest}")));
        }
        let parse = |value: &str| {
            value
                .parse::<i64>()
                .map_err(|_| NativeError::Failed(format!("bad DIAG value: {rest}")))
        };
        diagnostics.push(RuntimeDiagnostic {
            file: parse(values[3])?,
            origin: parse(values[4])?,
            subject: if with_subject {
                Some((parse(values[5])?, parse(values[6])?))
            } else {
                None
            },
            ..RuntimeDiagnostic::new(parse(values[0])?, parse(values[1])?, parse(values[2])?)
        });
    }
    Ok((parse_outcome(stdout)?, diagnostics))
}

pub(crate) fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

pub(crate) use crate::kernel::find_clang;

/// Monomorphizes `module` and builds it for the host: the executable runs on
/// this machine, whatever triple the module was compiled for.
pub(crate) fn host_module(module: &Module) -> Module {
    let mut module = klf_kir_passes::monomorphize(module);
    module.target = crate::kernel::host_triple()
        .parse()
        .expect("the host triple is a valid target");
    module
}

pub(crate) fn macos_sysroot() -> Option<String> {
    let output = Command::new("xcrun")
        .args(["--show-sdk-path"])
        .output()
        .ok()?;
    let path = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!path.is_empty()).then_some(path)
}

fn build_runtime() -> Result<PathBuf, String> {
    crate::kernel::kernel_archive(
        crate::kernel::KernelArchive::Runtime,
        crate::kernel::host_triple(),
    )
}
