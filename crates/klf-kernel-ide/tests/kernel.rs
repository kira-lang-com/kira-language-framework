//! Kernel file analysis: a definition package with an edited document, a
//! KIR file's outline and navigation, and a `.ridl` file's `@Source` links.

use klf_facts::Entity;
use klf_facts::lexical::Classifier;
use klf_ide::{AnalysisRequest, Analyzer, Cancellation, Snapshot};
use klf_kernel_ide::{KernelAnalyzer, languages, specs};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

#[test]
fn the_kernel_specs_are_valid() {
    for spec in [specs::klfd(), specs::kir(), specs::ridl()] {
        assert!(
            spec.validate().is_empty(),
            "{}: {:?}",
            spec.display_name,
            spec.validate()
        );
        assert!(Classifier::new(&spec).is_ok(), "{}", spec.display_name);
    }
}

#[test]
fn a_package_is_checked_with_its_open_documents() {
    let root = workspace().join("definitions/mini");
    let reader = root.join("reader.klfd");
    let broken = "function name_end() -> Int {\n    return missing\n}\n";
    let analysis = KernelAnalyzer.analyze(
        &AnalysisRequest {
            directory: root.clone(),
            arguments: vec![root.display().to_string()],
            documents: vec![(reader.clone(), broken.to_owned())],
            kir: false,
        },
        &Cancellation::new(),
    );
    assert_eq!(analysis.failure, None);
    let file = analysis.file(&reader).expect("reader.klfd is analyzed");
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.file == file),
        "the edited text is what is checked: {:?}",
        analysis.diagnostics
    );
    let language = analysis
        .file(&root.join("language.klfd"))
        .expect("language.klfd is analyzed");
    let outline = analysis.facts.document_symbols(language);
    assert_eq!(outline.len(), 1);
    assert_eq!(
        analysis
            .facts
            .symbol(outline[0].entity)
            .map(|symbol| symbol.name.as_str()),
        Some("Mini")
    );
    assert!(outline[0].children.len() > 4, "members of the construct");
}

#[test]
fn a_kir_file_navigates_values_blocks_and_functions() {
    let path = std::env::temp_dir().join(format!("klf-kernel-ide-{}.kir", std::process::id()));
    let text = "module @M lang \"test\" target \"aarch64-apple-darwin\" stage raw\n\
\n\
func private @one() -> i64 engine(runtime) {\n\
bb0():\n\
  %x = const.int i64, 1\n\
  br bb1(%x)\n\
bb1(%y: i64):\n\
  return %y\n\
}\n\
\n\
func private @two() -> i64 engine(runtime) {\n\
bb0():\n\
  %r = call @one()\n\
  return %r\n\
}\n";
    let analysis = Arc::new(KernelAnalyzer.analyze(
        &AnalysisRequest {
            directory: path.parent().expect("directory").to_path_buf(),
            arguments: vec![path.display().to_string()],
            documents: vec![(path.clone(), text.to_owned())],
            kir: false,
        },
        &Cancellation::new(),
    ));
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    let documents = Arc::new(HashMap::from([(path.clone(), Arc::from(text))]));
    let snapshot = Snapshot::new(Arc::clone(&analysis), Arc::new(languages()), documents);
    let offset = |needle: &str| u32::try_from(text.find(needle).expect("needle")).expect("offset");

    let use_of_x = offset("br bb1(%x)") + 8;
    let definition = snapshot.definition(&path, use_of_x);
    assert_eq!(definition[0].start, offset("%x ="));

    let block = offset("br bb1") + 3;
    assert_eq!(snapshot.definition(&path, block)[0].start, offset("bb1(%y"));

    let call = offset("@one()") + 1;
    assert_eq!(
        snapshot.definition(&path, call)[0].start,
        offset("@one() -> i64")
    );

    let outline = snapshot.document_symbols(&path);
    let names: Vec<&str> = outline.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, ["one", "two"]);
    assert!(outline[0].children.iter().any(|child| child.name == "bb1"));
    assert!(
        analysis
            .facts
            .symbol(Entity(u64::from(offset("%r ="))))
            .is_some()
    );
}

#[test]
fn ridl_sources_link_and_lead_to_the_providing_language() {
    let directory =
        std::env::temp_dir().join(format!("klf-kernel-ide-ridl-{}", std::process::id()));
    std::fs::create_dir_all(directory.join("src")).expect("directory");
    let c = directory.join("src/shapes.c");
    std::fs::write(
        &c,
        "#include <math.h>\n\ndouble area(double r) { return r * r; }\n",
    )
    .expect("source written");
    let path = directory.join("out/shapes.ridl");
    let text = "struct Point\n\n@Source(\"../src/shapes.c:3:8\")\nfunction area(r: F64) -> F64\n";
    let analysis = Arc::new(KernelAnalyzer.analyze(
        &AnalysisRequest {
            directory: directory.clone(),
            arguments: vec![path.display().to_string()],
            documents: vec![(path.clone(), text.to_owned())],
            kir: false,
        },
        &Cancellation::new(),
    ));
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    let documents = Arc::new(HashMap::from([(path.clone(), Arc::from(text))]));
    let snapshot = Snapshot::new(Arc::clone(&analysis), Arc::new(languages()), documents);
    let offset = |needle: &str| u32::try_from(text.find(needle).expect("needle")).expect("offset");
    let at_area = 26;

    let links = snapshot.links(&path);
    assert_eq!(links.len(), 1);
    assert_eq!(
        (links[0].0, links[0].1),
        (offset("../src"), offset("\")\n"))
    );
    for cursor in [offset("../src") + 3, offset("area(r") + 1] {
        let definition = snapshot.definition(&path, cursor);
        assert_eq!(definition.len(), 1, "at {cursor}");
        assert_eq!(definition[0].path, c);
        assert_eq!(
            (definition[0].start, definition[0].end),
            (at_area, at_area + 4)
        );
    }
    assert!(snapshot.definition(&path, offset("Point")).is_empty());
    std::fs::remove_dir_all(&directory).expect("cleaned up");
}
