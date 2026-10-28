//! A call into an imported RIDL module leads to the `.ridl` declaration it
//! was checked against (`KLF-EDITOR.md` section 15.3 item 2).
//!
//! A definition reads a RIDL module as the bindings
//! `<directory>/<module>.ridl#kira` that `klf.ridl.contract_bindings`
//! writes, so what it records is declared in that text, which is in no file
//! an editor can open. Each top-level declaration of the bindings is moved
//! to the declaration of the same name in the directory's `.ridl` files.

use klf_core::FileId;
use klf_facts::Facts;
use klf_services::Session;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const BINDINGS: &str = ".ridl#kira";

pub(crate) fn redirect(session: &Session, facts: &mut Facts, files: &mut Vec<(FileId, PathBuf)>) {
    for bindings in session.files() {
        let Some(contract) = bindings.name().as_str().strip_suffix("#kira") else {
            continue;
        };
        if !bindings.name().as_str().ends_with(BINDINGS) {
            continue;
        }
        let directory = Path::new(contract).parent().unwrap_or(Path::new(""));
        let declared = declarations(session, directory, files);
        let moved: Vec<_> = facts
            .symbols()
            .filter(|(_, symbol)| symbol.file == Some(bindings.id()) && symbol.container.is_none())
            .filter_map(|(entity, symbol)| Some((entity, *declared.get(&symbol.name)?)))
            .collect();
        for (entity, (file, start, end)) in moved {
            if let Some(symbol) = facts.symbol_mut(entity) {
                symbol.file = Some(file);
                (symbol.start, symbol.end) = (start, end);
                (symbol.selection_start, symbol.selection_end) = (start, end);
            }
        }
    }
}

/// The name span of each top-level declaration of the `.ridl` files in
/// `directory`, the first file in path order winning, with each file given
/// an id after those of `files`.
fn declarations(
    session: &Session,
    directory: &Path,
    files: &mut Vec<(FileId, PathBuf)>,
) -> HashMap<String, (FileId, u32, u32)> {
    let mut declared = HashMap::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return declared;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "ridl")
        })
        .collect();
    paths.sort();
    for path in paths {
        let Ok(text) = session.read_text(&path) else {
            continue;
        };
        let absolute = std::path::absolute(&path).unwrap_or(path);
        let file = match files.iter().find(|(_, known)| *known == absolute) {
            Some((file, _)) => *file,
            None => {
                let next = files
                    .iter()
                    .map(|(file, _)| file.as_u32() + 1)
                    .max()
                    .unwrap_or(0);
                let file = FileId::from_u32(next);
                files.push((file, absolute));
                file
            }
        };
        let Ok((module, spans)) = klf_ridl::parse_spanned(file, "", &text) else {
            continue;
        };
        for (item, span) in module.items.iter().zip(&spans) {
            if let Some(name) = span.name {
                declared
                    .entry(klf_ridl::emit::item_path_of(item))
                    .or_insert((file, name.start, name.end));
            }
        }
    }
    declared
}

#[cfg(test)]
mod tests {
    use super::redirect;
    use klf_facts::{Entity, Facts, Symbol};
    use klf_services::Session;
    use std::path::PathBuf;
    use std::str::FromStr;

    #[test]
    fn bindings_declarations_move_to_the_ridl() {
        let directory =
            std::env::temp_dir().join(format!("klf-ridl-declarations-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("directory");
        let ridl = directory.join("shapes.ridl");
        let text = "struct Point\nfunction area(r: F64) -> F64\n";
        std::fs::write(&ridl, text).expect("ridl written");
        let mut session =
            Session::new(klf_target::Target::from_str("aarch64-apple-darwin").unwrap());
        let source = session.add_file("main.kira", String::new());
        let bindings = session.add_file(
            format!("{}#kira", ridl.display()).as_str(),
            "function area(r: Float) -> Float { }".to_owned(),
        );
        let mut facts = Facts::new();
        let declare = |facts: &mut Facts, entity: u64, file, name: &str| {
            facts.declare_symbol(
                Entity(entity),
                Symbol {
                    name: name.to_owned(),
                    file: Some(file),
                    start: 9,
                    end: 13,
                    selection_start: 9,
                    selection_end: 13,
                    ..Symbol::default()
                },
            );
        };
        declare(&mut facts, 1, bindings, "area");
        declare(&mut facts, 2, bindings, "helper");
        declare(&mut facts, 3, source, "area");
        let mut files: Vec<_> = [source, bindings]
            .into_iter()
            .map(|file| (file, PathBuf::from(session.file(file).name().as_str())))
            .collect();
        redirect(&session, &mut facts, &mut files);

        let moved = facts.symbol(Entity(1)).expect("area");
        let file = moved.file.expect("a file");
        let path = &files
            .iter()
            .find(|(known, _)| *known == file)
            .expect("listed")
            .1;
        assert_eq!(path, &std::path::absolute(&ridl).unwrap());
        let start = text.find("area").unwrap() as u32;
        assert_eq!(
            (moved.selection_start, moved.selection_end),
            (start, start + 4)
        );
        assert_eq!(
            facts.symbol(Entity(2)).expect("helper").file,
            Some(bindings)
        );
        assert_eq!(facts.symbol(Entity(3)).expect("source").file, Some(source));
        std::fs::remove_dir_all(&directory).expect("cleaned up");
    }
}
