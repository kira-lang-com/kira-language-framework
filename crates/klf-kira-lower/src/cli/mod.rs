//! A definition's own commands (`KLF-CLI.md` sections 3 and 4).
//!
//! The KLF module provides the `Option`, `Flag` and `Argument` property
//! wrappers and the `NameSpecification` they take, and gives every package
//! that uses them those declarations. A `ParsableCommand` struct is ordinary
//! code: its `configuration` and its conformance are what the kernel reads
//! (`spec.rs`), and the package gains one generated function,
//! `__klf_command(command) -> Int`, which builds the selected command from the
//! parsed values and runs it (`entry.rs`).

mod entry;
pub(crate) mod spec;

use klf_core::{FileId, Name};
use klf_kira_syntax::ast::{DeclKind, File, MemberKind};
use klf_services::Session;
use klf_source::SourceFile;
use std::collections::HashSet;

pub(crate) const PARSABLE_COMMAND: &str = "ParsableCommand";
pub(crate) const OPTION: &str = "Option";
pub(crate) const FLAG: &str = "Flag";
pub(crate) const ARGUMENT: &str = "Argument";

/// The generated command entry the native trampoline and the VM call.
pub const COMMAND_ENTRY: &str = "__klf_command";

const PRELUDE: &str = "\
enum NameSpecification {
    Long
    Short
    ShortAndLong
    CustomLong(String)
    CustomShort(String)
    Custom(String, String)
}

@PropertyWrapper
struct Option<Value> {
    var wrappedValue: Value
    let name: NameSpecification = .Long
    let help: String = \"\"
    let valueName: String = \"\"
    let hidden: Bool = false
}

@PropertyWrapper
struct Flag<Value> {
    var wrappedValue: Value
    let name: NameSpecification = .Long
    let help: String = \"\"
    let hidden: Bool = false
}

@PropertyWrapper
struct Argument<Value> {
    var wrappedValue: Value
    let help: String = \"\"
    let valueName: String = \"\"
    let hidden: Bool = false
}
";

fn uses_cli(file: &File) -> bool {
    file.decls.iter().any(|decl| {
        let DeclKind::Struct(structure) = &decl.kind else {
            return false;
        };
        spec::conforms(structure)
            || structure.members.iter().any(|member| {
                member.annotations.iter().any(|annotation| {
                    matches!(
                        crate::wrappers::annotation_name(&annotation.path),
                        Some(OPTION | FLAG | ARGUMENT)
                    )
                })
            })
    })
}

/// Gives `file` the KLF module's command-line declarations it uses, takes out
/// what only the kernel reads, and adds the command entry.
pub(crate) fn prepare(session: &mut Session, mut file: File) -> File {
    if !uses_cli(&file) {
        return file;
    }
    let Some(commands) = spec::commands(session, &file) else {
        return file;
    };
    let declared: HashSet<Name> = file
        .decls
        .iter()
        .filter_map(|decl| match &decl.kind {
            DeclKind::Struct(declared) => Some(declared.name.name.clone()),
            DeclKind::Enum(declared) => Some(declared.name.name.clone()),
            _ => None,
        })
        .collect();
    let prelude = SourceFile::new(FileId::from_u32(0), "<klf-cli>", PRELUDE);
    for decl in klf_kira_syntax::parse(&prelude).file.decls {
        let name = match &decl.kind {
            DeclKind::Struct(declared) => &declared.name.name,
            DeclKind::Enum(declared) => &declared.name.name,
            _ => continue,
        };
        if !declared.contains(name) {
            file.decls.push(decl);
        }
    }
    for decl in &mut file.decls {
        let DeclKind::Struct(structure) = &mut decl.kind else {
            continue;
        };
        if !spec::conforms(structure) {
            continue;
        }
        structure.conforms.retain(|ty| {
            !matches!(&ty.kind, klf_kira_syntax::ast::TypeKind::Named { path, .. }
                if path.segments.len() == 1 && path.segments[0].name.as_str() == PARSABLE_COMMAND)
        });
        structure.members.retain(|member| {
            !matches!(&member.kind, MemberKind::Field { name, .. } if name.name.as_str() == "configuration")
        });
    }
    file.decls.extend(entry::declarations(&commands));
    file
}
