//! The shader driver's command line (`KLF-KSL.md` section 5.7).

use klf_shader::Target;
use klf_shader_passes::{Bounds, IntegerDivision};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Build,
    Check,
}

/// One output printed instead of written (`--emit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Emit {
    Source,
    Reflection,
    Ksir,
    Digest,
}

#[derive(Clone, Debug)]
pub struct Invocation {
    pub command: Command,
    pub paths: Vec<PathBuf>,
    /// The requested targets; empty means every target the language has.
    pub targets: Vec<Target>,
    pub out: Option<PathBuf>,
    pub emit: Option<Emit>,
    pub keep_overrides: bool,
    pub overrides: BTreeMap<String, String>,
    pub bounds: Bounds,
    pub integer_division: IntegerDivision,
    pub spirv_minor: u8,
    pub json: bool,
    pub debug_info: bool,
}

fn value<'a>(rest: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<&'a str, String> {
    rest.next()
        .map(String::as_str)
        .ok_or_else(|| format!("`{flag}` needs a value"))
}

fn take<'a>(
    inline: Option<&'a str>,
    flag: &str,
    rest: &mut std::slice::Iter<'a, String>,
) -> Result<&'a str, String> {
    match inline {
        Some(value) => Ok(value),
        None => value(rest, flag),
    }
}

fn choice<T: Copy>(flag: &str, given: &str, choices: &[(&str, T)]) -> Result<T, String> {
    choices
        .iter()
        .find(|(name, _)| *name == given)
        .map(|(_, value)| *value)
        .ok_or_else(|| {
            let names: Vec<&str> = choices.iter().map(|(name, _)| *name).collect();
            format!("`{flag}` takes {}, not `{given}`", names.join(", "))
        })
}

/// Splits `--flag=value` into its two parts.
fn split(argument: &str) -> (&str, Option<&str>) {
    match argument.split_once('=') {
        Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
        _ => (argument, None),
    }
}

/// Reads `arguments`, the process arguments after the program name.
///
/// # Errors
/// Returns a message for an unknown command, flag or value.
pub fn parse(arguments: &[String]) -> Result<Invocation, String> {
    let mut rest = arguments.iter();
    let command = match rest.next().map(String::as_str) {
        Some("build") => Command::Build,
        Some("check") => Command::Check,
        Some(other) => return Err(format!("unknown command `{other}`")),
        None => return Err("no command".to_owned()),
    };
    let mut invocation = Invocation {
        command,
        paths: Vec::new(),
        targets: Vec::new(),
        out: None,
        emit: None,
        keep_overrides: false,
        overrides: BTreeMap::new(),
        bounds: Bounds::Unchecked,
        integer_division: IntegerDivision::Defined,
        spirv_minor: 3,
        json: false,
        debug_info: false,
    };
    while let Some(argument) = rest.next() {
        let (flag, inline) = split(argument);
        match flag {
            "--debug-info" => invocation.debug_info = true,
            _ if flag.starts_with("--") => {
                setting(&mut invocation, flag, take(inline, flag, &mut rest)?)?;
            }
            _ => invocation.paths.push(PathBuf::from(argument)),
        }
    }
    if invocation.emit.is_some() && (invocation.paths.len() != 1 || invocation.targets.len() > 1) {
        return Err("`--emit` prints one output for one file and one target".to_owned());
    }
    Ok(invocation)
}

/// Applies one flag that takes a value.
fn setting(invocation: &mut Invocation, flag: &str, given: &str) -> Result<(), String> {
    match flag {
        "--target" => {
            for name in given.split(',') {
                let target = Target::from_name(name)
                    .ok_or_else(|| format!("KLF-DRV-0030: unknown target `{name}`"))?;
                if !invocation.targets.contains(&target) {
                    invocation.targets.push(target);
                }
            }
        }
        "--out" => invocation.out = Some(PathBuf::from(given)),
        "--emit" => {
            invocation.emit = Some(choice(
                flag,
                given,
                &[
                    ("source", Emit::Source),
                    ("reflection", Emit::Reflection),
                    ("ksir", Emit::Ksir),
                    ("digest", Emit::Digest),
                ],
            )?);
        }
        "--overrides" => {
            invocation.keep_overrides = choice(flag, given, &[("bake", false), ("keep", true)])?;
        }
        "--override" => {
            let (name, value) = given
                .split_once('=')
                .ok_or_else(|| format!("`--override` takes name=value, not `{given}`"))?;
            invocation
                .overrides
                .insert(name.to_owned(), value.to_owned());
        }
        "--bounds" => {
            invocation.bounds = choice(
                flag,
                given,
                &[
                    ("unchecked", Bounds::Unchecked),
                    ("restrict", Bounds::Restrict),
                    ("read-zero-skip-write", Bounds::ReadZeroSkipWrite),
                ],
            )?;
        }
        "--integer-division" => {
            invocation.integer_division = choice(
                flag,
                given,
                &[
                    ("defined", IntegerDivision::Defined),
                    ("native", IntegerDivision::Native),
                ],
            )?;
        }
        "--msl-version" => {
            choice(
                flag,
                given,
                &[("2.4", ()), ("3.0", ()), ("3.1", ()), ("3.2", ())],
            )?;
        }
        "--spirv-version" => {
            invocation.spirv_minor = choice(
                flag,
                given,
                &[
                    ("1.3", 3),
                    ("1.4", 4),
                    ("1.5", 5),
                    ("1.6", 6),
                    ("1.0", 0),
                    ("1.1", 1),
                    ("1.2", 2),
                ],
            )?;
        }
        "--hlsl-model" => {
            let models = [
                "5_0", "5_1", "6_0", "6_1", "6_2", "6_3", "6_4", "6_5", "6_6", "6_7", "6_8",
            ];
            let pairs: Vec<(&str, ())> = models.iter().map(|model| (*model, ())).collect();
            choice(flag, given, &pairs)?;
        }
        "--diagnostics" => {
            invocation.json = choice(flag, given, &[("text", false), ("json", true)])?;
        }
        _ => return Err(format!("unknown flag `{flag}`")),
    }
    Ok(())
}

#[must_use]
pub fn usage(driver: &str) -> String {
    format!(
        "usage:\n  {driver} build [paths] [--target T1,T2] [--out DIR]\n  {driver} build FILE --target T --emit source|reflection|ksir|digest\n  {driver} check [paths]\n  {driver} lsp\n\nflags: --overrides bake|keep, --override NAME=VALUE, --bounds unchecked|restrict|read-zero-skip-write,\n       --integer-division defined|native, --msl-version V, --spirv-version V, --hlsl-model M,\n       --diagnostics text|json, --debug-info"
    )
}
