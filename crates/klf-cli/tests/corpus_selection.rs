use std::process::Command;

#[test]
fn requesting_a_missing_corpus_fails() {
    let package = std::env::temp_dir().join(format!("klf-no-corpus-{}", std::process::id()));
    std::fs::create_dir_all(&package).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_klf"))
        .arg("test")
        .arg(&package)
        .args(["--corpus", "missing"])
        .output()
        .unwrap();
    std::fs::remove_dir_all(package).unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no corpus manifest"));
}
