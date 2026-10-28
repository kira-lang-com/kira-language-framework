//! `klf ffi` subcommands: `gen` a `.klffi` from a C header, `emit` a parsed
//! `.klffi`, and `decls` to show the KIR foreign declarations it maps to.

use std::path::Path;
use std::process::ExitCode;

use klf_klffi::{Environment, Predefined, emit, foreign_declarations, generate, parse};
use std::path::PathBuf;

pub fn command(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("gen") => gen_cmd(&args[1..]),
        Some("emit") => emit_cmd(&args[1..]),
        Some("decls") => decls(&args[1..]),
        _ => {
            klf_diag::err!(
                "usage:\n  klf ffi gen <header.h> [--library NAME] [--target TRIPLE] [-I DIR]... [-o OUT.klffi]\n  klf ffi emit <file.klffi>\n  klf ffi decls <file.klffi>"
            );
            ExitCode::FAILURE
        }
    }
}

fn gen_cmd(args: &[String]) -> ExitCode {
    let mut header = None;
    let mut library = None;
    let mut output = None;
    let mut target = None;
    let mut include_dirs = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--library" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    klf_diag::err!("klf ffi gen: `--library` needs a value");
                    return ExitCode::FAILURE;
                };
                library = Some(value.clone());
            }
            "--target" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    klf_diag::err!("klf ffi gen: `--target` needs a value");
                    return ExitCode::FAILURE;
                };
                target = Some(value.clone());
            }
            "-I" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    klf_diag::err!("klf ffi gen: `-I` needs a directory");
                    return ExitCode::FAILURE;
                };
                include_dirs.push(PathBuf::from(value));
            }
            "-o" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    klf_diag::err!("klf ffi gen: `-o` needs a value");
                    return ExitCode::FAILURE;
                };
                output = Some(value.clone());
            }
            flag if flag.starts_with('-') => {
                klf_diag::err!("klf ffi gen: unknown flag `{flag}`");
                return ExitCode::FAILURE;
            }
            value => header = Some(value.to_string()),
        }
        index += 1;
    }
    let Some(header_path) = header else {
        klf_diag::err!("klf ffi gen: expected a C header");
        return ExitCode::FAILURE;
    };
    let text = match std::fs::read_to_string(&header_path) {
        Ok(text) => text,
        Err(e) => {
            klf_diag::err!("klf ffi gen: cannot read `{header_path}`: {e}");
            return ExitCode::FAILURE;
        }
    };
    let library = library.unwrap_or_else(|| stem(&header_path));
    let triple = target.unwrap_or_else(|| klf_target::host_triple().to_owned());
    let Some(predefined) = Predefined::for_triple(&triple) else {
        klf_diag::err!("klf ffi gen: no predefined macros for the target `{triple}`");
        return ExitCode::FAILURE;
    };
    let environment = Environment {
        predefined: &predefined,
        include_dirs: &include_dirs,
    };
    let path = PathBuf::from(&header_path);
    let (module, remarks) = match generate(&text, &path, &library, environment, &mut |candidate| {
        std::fs::read_to_string(candidate).ok()
    }) {
        Ok(generated) => generated,
        Err(error) => {
            klf_diag::err!("klf ffi gen: {error}");
            return ExitCode::FAILURE;
        }
    };
    for remark in &remarks {
        klf_diag::err!("remark: {remark}");
    }
    let out = emit(&module);
    if let Some(path) = output {
        if let Err(e) = std::fs::write(&path, out) {
            klf_diag::err!("klf ffi gen: cannot write `{path}`: {e}");
            return ExitCode::FAILURE;
        }
    } else {
        klf_diag::raw!("{out}");
    }
    ExitCode::SUCCESS
}

fn emit_cmd(args: &[String]) -> ExitCode {
    let Some(module) = load(args.first()) else {
        return ExitCode::FAILURE;
    };
    klf_diag::raw!("{}", emit(&module));
    ExitCode::SUCCESS
}

fn decls(args: &[String]) -> ExitCode {
    let Some(module) = load(args.first()) else {
        return ExitCode::FAILURE;
    };
    let rules = klf_target::host_triple()
        .parse::<klf_target::Target>()
        .ok()
        .and_then(|target| target.c_layout());
    let Some(rules) = rules else {
        klf_diag::err!(
            "klf ffi decls: no C rules for the host `{}`",
            klf_target::host_triple()
        );
        return ExitCode::FAILURE;
    };
    for function in foreign_declarations(&module, &rules) {
        klf_diag::out!("{function}", function = one_line(&function));
    }
    ExitCode::SUCCESS
}

fn one_line(function: &klf_kir::Function) -> String {
    let params: Vec<String> = function
        .parameters
        .iter()
        .map(|p| format!("%{}: {}", p.name, p.ty))
        .collect();
    format!(
        "func private foreign @{}({}) -> {} engine(native)",
        function.name,
        params.join(", "),
        function.result
    )
}

fn load(path: Option<&String>) -> Option<klf_klffi::FfiModule> {
    let path = path?;
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            klf_diag::err!("klf ffi: cannot read `{path}`: {e}");
            return None;
        }
    };
    match parse(&text) {
        Ok(module) => Some(module),
        Err(error) => {
            klf_diag::err!(
                "{path}:{}: error[{}]: {}",
                error.line,
                error.code,
                error.message
            );
            None
        }
    }
}

fn stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("c")
        .to_owned()
}
