use super::{CorpusManifest, EditorManifest, read_corpora, read_editor_corpora};
use std::path::Path;

fn contains_manifest(directory: &Path, only: Option<&str>) -> bool {
    std::fs::read_dir(directory).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let path = entry.path();
            path.is_file()
                && path.extension().is_some_and(|extension| extension == "kcore" || extension == "klfd")
                && only.is_none_or(|name| path.file_stem().is_some_and(|stem| stem == name))
        })
    })
}

pub(super) fn has_corpus(package: &Path) -> bool {
    let directory = package.join("corpus");
    contains_manifest(&directory, None) || contains_manifest(&directory.join("editor"), None)
}

pub(super) fn read(package: &Path, only: Option<&str>) -> Result<(Vec<CorpusManifest>, Vec<EditorManifest>), String> {
    let directory = package.join("corpus");
    let editor = read_editor_corpora(package, only)?;
    let programs = if contains_manifest(&directory, only) {
        read_corpora(package, only)?
    } else if editor.is_empty() && only.is_some() {
        return Err(format!("no corpus manifest named {} in {}", only.unwrap(), directory.display()));
    } else {
        Vec::new()
    };
    Ok((programs, editor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_program_and_editor_manifests_by_name_and_extension() {
        let root = std::env::temp_dir().join(format!("klf-corpus-selection-{}", std::process::id()));
        let corpus = root.join("corpus");
        let editor = corpus.join("editor");
        std::fs::create_dir_all(&editor).unwrap();
        assert!(!has_corpus(&root));
        std::fs::write(corpus.join("program.kcore"), "construct Programs() extends Corpus { let cases: [CorpusCase] = [] }").unwrap();
        std::fs::write(editor.join("editing.klfd"), "construct Editing() extends EditorCorpus { let cases: [EditorCase] = [] }").unwrap();
        assert!(has_corpus(&root));
        let (programs, editors) = read(&root, Some("editing")).unwrap();
        assert!(programs.is_empty());
        assert_eq!(editors.len(), 1);
        let (programs, editors) = read(&root, Some("program")).unwrap();
        assert_eq!(programs.len(), 1);
        assert!(editors.is_empty());
        let (programs, editors) = read(&root, None).unwrap();
        assert_eq!((programs.len(), editors.len()), (1, 1));
        assert!(read(&root, Some("missing")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
