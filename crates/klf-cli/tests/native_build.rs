use std::{fs, process::Command};

#[test]
fn builds_and_runs_native_binary_from_kir() {
    if !klf_driver::toolchain_available() {
        eprintln!("skipping native build test: native toolchain is unavailable");
        return;
    }

    let temp_dir = std::env::temp_dir().join(format!("klf-native-build-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("create temporary directory");

    let input = temp_dir.join("program.kir");
    let output = temp_dir.join("program");
    fs::write(
        &input,
        format!(
            r#"module @Build lang "test" target "{}" stage raw

func private @main() -> i64 engine(runtime) {{
bb0():
  %answer = const.int i64, 42
  return %answer
}}
"#,
            klf_driver::host_triple()
        ),
    )
    .expect("write KIR input");

    let build = Command::new(env!("CARGO_BIN_EXE_klf"))
        .arg("build")
        .arg(&input)
        .args(["--emit", "bin", "-o"])
        .arg(&output)
        .output()
        .expect("run klf build");
    assert!(
        build.status.success(),
        "klf build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&output)
        .output()
        .expect("run generated native binary");
    assert!(
        run.status.success(),
        "generated binary failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "RET 42");
}
