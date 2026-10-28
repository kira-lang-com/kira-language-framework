//! `klf abi` subcommands: freeze, apply, retire (section 24.4).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use klf_core::FileId;
use klf_ridl::abi::{
    AbiHistory, Freeze, Surface, apply as apply_stamps, freeze, mint_uuid_v4, record_from_text,
    record_to_text,
};
use klf_ridl::emit;
use klf_ridl::emit::item_path_of;
use klf_ridl::module::Module;
use klf_ridl::parse;

pub fn command(args: &[String]) -> ExitCode {
    let Some(sub) = args.first().map(String::as_str) else {
        klf_diag::err!("usage: klf abi freeze|apply|retire <file.ridl> [uuid]");
        return ExitCode::FAILURE;
    };
    match sub {
        "freeze" => run(args.get(1), cmd_freeze),
        "apply" => run(args.get(1), cmd_apply),
        "retire" => cmd_retire(args.get(1), args.get(2)),
        other => {
            klf_diag::err!("klf abi: unknown subcommand `{other}`");
            ExitCode::FAILURE
        }
    }
}

fn run(path: Option<&String>, f: impl Fn(&str, &Module, &AbiHistory) -> ExitCode) -> ExitCode {
    let Some(path) = path else {
        klf_diag::err!("klf abi: expected a .ridl file");
        return ExitCode::FAILURE;
    };
    let Some(module) = load(path) else {
        return ExitCode::FAILURE;
    };
    let history = match load_history(&abi_dir(path)) {
        Ok(h) => h,
        Err(e) => {
            klf_diag::err!("klf abi: {e}");
            return ExitCode::FAILURE;
        }
    };
    f(path, &module, &history)
}

fn cmd_freeze(path: &str, module: &Module, history: &AbiHistory) -> ExitCode {
    let surface = Surface::from_module(module);
    let uuid = mint_uuid_v4();
    match freeze(&surface, history, &uuid) {
        Ok(Freeze::Unchanged) => {
            klf_diag::out!("abi: no change; nothing to freeze");
            ExitCode::SUCCESS
        }
        Ok(Freeze::New {
            record,
            superseded_origin,
        }) => {
            let dir = abi_dir(path);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                klf_diag::err!("klf abi: cannot create `{}`: {e}", dir.display());
                return ExitCode::FAILURE;
            }
            let file = dir.join(format!("{uuid}.ridlsig"));
            let text = record_to_text(&record, &superseded_origin);
            if let Err(e) = std::fs::write(&file, text) {
                klf_diag::err!("klf abi: cannot write `{}`: {e}", file.display());
                return ExitCode::FAILURE;
            }
            klf_diag::out!("abi: froze {} new shape(s) as {uuid}", record.added.len());
            ExitCode::SUCCESS
        }
        Err(e) => {
            klf_diag::err!("klf abi: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_apply(path: &str, module: &Module, history: &AbiHistory) -> ExitCode {
    let surface = Surface::from_module(module);
    match freeze(&surface, history, "0") {
        Ok(Freeze::New {
            superseded_origin, ..
        }) => {
            if superseded_origin.is_empty() {
                klf_diag::out!("abi: nothing to stamp");
                return ExitCode::SUCCESS;
            }
            let mut edited = module.clone();
            let stamped = apply_stamps(&mut edited, &superseded_origin);
            if let Err(e) = std::fs::write(path, emit(&edited)) {
                klf_diag::err!("klf abi: cannot rewrite `{path}`: {e}");
                return ExitCode::FAILURE;
            }
            klf_diag::out!("abi: stamped {stamped} superseded shape(s) in {path}");
            ExitCode::SUCCESS
        }
        Ok(Freeze::Unchanged) => {
            klf_diag::out!("abi: nothing to stamp");
            ExitCode::SUCCESS
        }
        Err(e) => {
            klf_diag::err!("klf abi: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_retire(path: Option<&String>, uuid: Option<&String>) -> ExitCode {
    let (Some(path), Some(uuid)) = (path, uuid) else {
        klf_diag::err!("usage: klf abi retire <file.ridl> <uuid>");
        return ExitCode::FAILURE;
    };
    let dir = abi_dir(path);
    let record = dir.join(format!("{uuid}.ridlsig"));
    if !record.exists() {
        klf_diag::err!("klf abi: no record `{}`", record.display());
        return ExitCode::FAILURE;
    }
    let unsupported = dir.join("unsupported");
    if let Err(e) = std::fs::create_dir_all(&unsupported) {
        klf_diag::err!("klf abi: {e}");
        return ExitCode::FAILURE;
    }
    let dest = unsupported.join(format!("{uuid}.ridlsig"));
    if let Err(e) = std::fs::rename(&record, &dest) {
        klf_diag::err!("klf abi: cannot retire: {e}");
        return ExitCode::FAILURE;
    }
    klf_diag::out!("abi: retired {uuid}");
    ExitCode::SUCCESS
}

fn abi_dir(path: &str) -> PathBuf {
    Path::new(path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("abi")
}

fn load(path: &str) -> Option<Module> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| klf_diag::err!("klf abi: cannot read `{path}`: {e}"))
        .ok()?;
    let name = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module");
    parse(FileId::from_u32(0), name, &text)
        .map_err(|d| klf_diag::err!("{path}: {d}"))
        .ok()
}

fn load_history(dir: &Path) -> Result<AbiHistory, String> {
    let mut history = AbiHistory::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(history);
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ridlsig"))
        .collect();
    files.sort();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("cannot read `{}`: {e}", file.display()))?;
        let record = record_from_text(&text, shape_path)?;
        history.records.push(record);
    }
    Ok(history)
}

fn shape_path(shape: &str) -> Option<String> {
    let module = parse(FileId::from_u32(0), "shape", shape).ok()?;
    module.items.first().map(item_path_of)
}
