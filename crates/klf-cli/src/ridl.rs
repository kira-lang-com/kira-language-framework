//! `klf ridl` subcommands: check, emit, layout, mangle, adapters, bindgen.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use klf_core::FileId;
use klf_ridl::layout::LayoutEngine;
use klf_ridl::mangle::{descriptor_symbol, function_symbol};
use klf_ridl::module::{AnnValue, ExtendBody, ItemKind, Module};
use klf_ridl::{Type, bindgen, emit, parse};

pub fn command(args: &[String]) -> ExitCode {
    let Some(sub) = args.first().map(String::as_str) else {
        klf_diag::err!(
            "usage: klf ridl check|emit|layout|mangle <file.ridl> | adapters --target <triple> [-o out.ll] <file.ridl> | bindgen [--lang rust|kira|swift] <file.ridl>"
        );
        return ExitCode::FAILURE;
    };
    if sub == "adapters" {
        return adapters(args);
    }
    let parsed = if sub == "bindgen" {
        bindgen_arguments(args)
    } else {
        args.get(1)
            .cloned()
            .map(|path| ("rust".to_owned(), path))
            .ok_or_else(|| format!("klf ridl {sub}: expected a .ridl file"))
    };
    let (language, path) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            klf_diag::err!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(module) = load(&path) else {
        return ExitCode::FAILURE;
    };
    match sub {
        "check" => klf_diag::out!("Check: Ok {path}"),
        "emit" => klf_diag::raw!("{}", emit(&module)),
        "layout" => print_layout(&module),
        "mangle" => print_mangle(&module),
        "bindgen" if language == "rust" => klf_diag::raw!("{}", bindgen::rust_bindings(&module)),
        "bindgen" if language == "swift" => klf_diag::raw!("{}", bindgen::swift_bindings(&module)),
        "bindgen" if language == "kira" => klf_diag::raw!("{}", bindgen::kira_bindings(&module)),
        "bindgen" => {
            klf_diag::err!("klf ridl bindgen: unsupported language `{language}`");
            return ExitCode::FAILURE;
        }
        other => {
            klf_diag::err!("klf ridl: unknown subcommand `{other}`");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn bindgen_arguments(args: &[String]) -> Result<(String, String), String> {
    let mut language = "rust".to_owned();
    let mut path = None;
    let mut rest = args.iter().skip(1);
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--lang" => {
                language.clone_from(rest.next().ok_or_else(|| {
                    "klf ridl bindgen: --lang needs rust, kira or swift".to_owned()
                })?);
            }
            option if option.starts_with('-') => {
                return Err(format!("klf ridl bindgen: unknown option `{option}`"));
            }
            _ if path.is_none() => path = Some(argument.clone()),
            other => return Err(format!("klf ridl bindgen: unexpected argument `{other}`")),
        }
    }
    let path = path.ok_or_else(|| "klf ridl bindgen: expected a .ridl file".to_owned())?;
    Ok((language, path))
}

/// `klf ridl adapters`: the LLVM IR of a C-view adapter for every export,
/// named by [`bindgen::caller_symbol`], for a language that calls the module
/// through the C convention. Compile it with clang and link it with the
/// providing library.
fn adapters(args: &[String]) -> ExitCode {
    let mut target = None;
    let mut output = None;
    let mut path = None;
    let mut rest = args.iter().skip(1);
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--target" => target = rest.next().cloned(),
            "-o" => output = rest.next().cloned(),
            option if option.starts_with('-') => {
                klf_diag::err!("klf ridl adapters: unknown option `{option}`");
                return ExitCode::FAILURE;
            }
            _ if path.is_none() => path = Some(argument.clone()),
            other => {
                klf_diag::err!("klf ridl adapters: unexpected argument `{other}`");
                return ExitCode::FAILURE;
            }
        }
    }
    let (Some(target), Some(path)) = (target, path) else {
        klf_diag::err!("klf ridl adapters: expected --target <triple> and a .ridl file");
        return ExitCode::FAILURE;
    };
    let Some(module) = load(&path) else {
        return ExitCode::FAILURE;
    };
    let mut callers = BTreeMap::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Function(function) => {
                callers.insert(
                    function.name.clone(),
                    bindgen::caller_symbol(&module.name, &function.name),
                );
            }
            ItemKind::Extend(extend) => {
                let (Type::Nominal { name, .. }, ExtendBody::Methods(methods)) =
                    (&extend.target, &extend.body)
                else {
                    continue;
                };
                for method in methods {
                    let path = format!("{name}.{}", method.name);
                    callers.insert(path.clone(), bindgen::caller_symbol(&module.name, &path));
                }
            }
            _ => {}
        }
    }
    let source = match klf_ridl::llvm::adapters(
        &module,
        &callers,
        &target,
        &|name| name.to_owned(),
        false,
    ) {
        Ok(source) => source,
        Err(error) => {
            klf_diag::err!("klf ridl adapters: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(output) = output {
        if let Err(error) = std::fs::write(&output, source) {
            klf_diag::err!("klf ridl adapters: cannot write `{output}`: {error}");
            return ExitCode::FAILURE;
        }
    } else {
        klf_diag::raw!("{source}");
    }

    ExitCode::SUCCESS
}

fn load(path: &str) -> Option<Module> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            klf_diag::err!("klf ridl: cannot read `{path}`: {e}");
            return None;
        }
    };
    let name = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module");
    match parse(FileId::from_u32(0), name, &text) {
        Ok(module) => Some(module),
        Err(diag) => {
            klf_diag::err!("{path}: {diag}");
            None
        }
    }
}

fn print_layout(module: &Module) {
    let registry = module.registry();
    let engine = LayoutEngine::new(&registry, 8);
    for item in &module.items {
        let (name, generic) = match &item.kind {
            ItemKind::Struct(s) => (s.name.clone(), !s.generics.is_empty()),
            ItemKind::Enum(e) => (e.name.clone(), !e.generics.is_empty()),
            _ => continue,
        };
        // A generic declaration has no layout of its own; each named
        // instance it declares has one (section 23.5).
        let types: Vec<Type> = if generic {
            item.annotations
                .iter()
                .filter(|annotation| annotation.name == "Instance")
                .map(|annotation| Type::Nominal {
                    name: name.clone(),
                    args: annotation
                        .args
                        .iter()
                        .filter_map(|arg| match &arg.value {
                            AnnValue::Ident(argument) => Some(Type::nominal(argument)),
                            AnnValue::Type(argument) => Some(argument.clone()),
                            AnnValue::String(_) => None,
                        })
                        .collect(),
                })
                .collect()
        } else {
            vec![Type::nominal(&name)]
        };
        for ty in types {
            let shown = klf_ridl::emit::type_text(&ty).replace(' ', "");
            match engine.layout_of(&ty) {
                Ok(l) => klf_diag::out!("{shown}: size {} align {}", l.size, l.align),
                Err(e) => klf_diag::out!("{shown}: {e}"),
            }
        }
    }
}

fn print_mangle(module: &Module) {
    for item in &module.items {
        match &item.kind {
            ItemKind::Function(f) => {
                klf_diag::out!("{}", function_symbol(&module.name, &f.name, None));
            }
            // A generic declaration exports no descriptor of its own.
            ItemKind::Struct(s) if s.generics.is_empty() => {
                klf_diag::out!("{}", descriptor_symbol(&module.name, &s.name));
            }
            ItemKind::Enum(e) if e.generics.is_empty() => {
                klf_diag::out!("{}", descriptor_symbol(&module.name, &e.name));
            }
            _ => {}
        }
    }
}
