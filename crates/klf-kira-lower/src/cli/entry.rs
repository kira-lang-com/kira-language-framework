//! The generated `__klf_command(command: Int) -> Int`.
//!
//! For the command numbered `command` it reads each argument slot through the
//! `klf.cli.*` services, falls back to the property's initial value when the
//! slot is empty, builds the struct and returns what its `run` returns. It is
//! written as KLF Kira and parsed, then each initial value is spliced in as
//! the syntax the definition wrote, so a mistake in one is reported where it
//! is. A choice reaches the struct through `__klf_choice_<Enum>(index)`.

use super::COMMAND_ENTRY;
use super::spec::{CliField, CommandDecl, Scalar, Shape};
use crate::wrappers::type_text;
use klf_core::{FileId, Name};
use klf_kira_syntax::ast::{Block, Decl, DeclKind, Expr, ExprKind, StmtKind};
use klf_source::SourceFile;
use std::collections::BTreeMap;
use std::fmt::Write as _;

const INIT: &str = "__klf_init";

/// The entry and choice helpers for `commands`, or nothing when no command
/// runs.
pub(super) fn declarations(commands: &[CommandDecl]) -> Vec<Decl> {
    if !commands.iter().any(|command| command.runs) {
        return Vec::new();
    }
    let mut inits = Vec::new();
    let mut choices = BTreeMap::new();
    let mut source = format!("function {COMMAND_ENTRY}(command: Int) -> Int {{\n");
    for command in commands.iter().filter(|command| command.runs) {
        let _ = writeln!(source, "    if command == {} {{", command.index);
        let mut literal = Vec::new();
        for (number, field) in command.fields.iter().enumerate() {
            let local = format!("__v{number}");
            field_source(&mut source, field, &local, number, &mut inits, &mut choices);
            literal.push(format!("{} = {local}", field.name));
        }
        let _ = writeln!(
            source,
            "        let __command = {} {{ {} }}\n        return __command.run()\n    }}",
            command.name,
            literal.join(", ")
        );
    }
    source.push_str("    return -1\n}\n");
    for (name, cases) in &choices {
        let _ = writeln!(
            source,
            "function __klf_choice_{name}(index: Int) -> {name} {{"
        );
        for (index, case) in cases.iter().enumerate().skip(1) {
            let _ = writeln!(source, "    if index == {index} {{ return .{case} }}");
        }
        let _ = writeln!(source, "    return .{}\n}}", cases[0]);
    }
    let file = SourceFile::new(FileId::from_u32(0), "<klf-command>", source.as_str());
    let mut decls = klf_kira_syntax::parse(&file).file.decls;
    for decl in &mut decls {
        if let DeclKind::Function(function) = &mut decl.kind
            && let Some(body) = &mut function.body
        {
            splice(body, &inits);
        }
    }
    decls
}

/// Declares `local` holding the value of `field`.
fn field_source(
    source: &mut String,
    field: &CliField,
    local: &str,
    number: usize,
    inits: &mut Vec<Expr>,
    choices: &mut BTreeMap<Name, Vec<Name>>,
) {
    let ty = type_text(&field.ty);
    let mut initial = |fallback: &str| match &field.init {
        Some(init) => {
            inits.push(init.clone());
            format!("{INIT}({})", inits.len() - 1)
        }
        None => fallback.to_owned(),
    };
    let Some((slot, scalar, shape)) = &field.value else {
        let _ = writeln!(source, "        var {local}: {ty} = {}", initial(""));
        return;
    };
    if let Scalar::Choice(name, cases) = scalar {
        choices.insert(name.clone(), cases.clone());
    }
    let element = |index: &str| match scalar {
        Scalar::Bool => format!("__klf_cli_int({slot}, {index}) != 0"),
        Scalar::Int => format!("__klf_cli_int({slot}, {index})"),
        Scalar::String => format!("__klf_cli_string({slot}, {index})"),
        Scalar::Choice(name, _) => format!("__klf_choice_{name}(__klf_cli_int({slot}, {index}))"),
    };
    let given = format!("__klf_cli_count({slot}) > 0");
    match shape {
        Shape::Single if field.init.is_none() => {
            let flag = match scalar {
                Scalar::Bool => Some("false"),
                Scalar::Int => Some("0"),
                _ => None,
            };
            match flag {
                Some(zero) => {
                    let _ = writeln!(
                        source,
                        "        var {local}: {ty} = {zero}\n        if {given} {{ {local} = {} }}",
                        element("0")
                    );
                }
                None => {
                    let _ = writeln!(source, "        let {local}: {ty} = {}", element("0"));
                }
            }
        }
        Shape::Single => {
            let _ = writeln!(
                source,
                "        var {local}: {ty} = {}\n        if {given} {{ {local} = {} }}",
                initial(""),
                element("0")
            );
        }
        Shape::Optional => {
            let _ = writeln!(
                source,
                "        var {local}: {ty} = {}\n        if {given} {{ {local} = .Some({}) }}",
                initial(".None"),
                element("0")
            );
        }
        Shape::List => {
            let index = format!("__i{number}");
            let list = format!("__l{number}");
            let _ = writeln!(
                source,
                "        var {local}: {ty} = {}\n        if {given} {{\n            var {list}: {ty} = []\n            var {index} = 0\n            while {index} < __klf_cli_count({slot}) {{\n                {list}.append({})\n                {index} = {index} + 1\n            }}\n            {local} = {list}\n        }}",
                initial("[]"),
                element(&index)
            );
        }
    }
}

/// Replaces each `__klf_init(k)` binding value with the `k`th initial value.
fn splice(block: &mut Block, inits: &[Expr]) {
    for stmt in &mut block.stmts {
        match &mut stmt.kind {
            StmtKind::Let { value, .. } => {
                if let Some(init) = placeholder(value).and_then(|index| inits.get(index)) {
                    *value = init.clone();
                }
            }
            StmtKind::If {
                branches,
                else_block,
            } => {
                for (_, body) in branches {
                    splice(body, inits);
                }
                if let Some(body) = else_block {
                    splice(body, inits);
                }
            }
            _ => {}
        }
    }
}

fn placeholder(value: &Expr) -> Option<usize> {
    let ExprKind::Call { callee, args } = &value.kind else {
        return None;
    };
    let ExprKind::Path(path) = &callee.kind else {
        return None;
    };
    if path.segments.len() != 1 || path.segments[0].name.as_str() != INIT {
        return None;
    }
    let [argument] = args.as_slice() else {
        return None;
    };
    match &argument.value.kind {
        ExprKind::Int(text) => text.as_str().parse().ok(),
        _ => None,
    }
}
