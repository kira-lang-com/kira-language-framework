//! Bootstrap Kira, compile a Kira frontend, and run the program it builds.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::io::Write;

fn run(command: &mut Command) -> Output {
    let output = command.output().expect("start command");
    assert!(
        output.status.success(),
        "command failed: {command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn kcore_bootstraps_a_kira_frontend_through_ridl() {
    let root = workspace();
    let directory = std::env::temp_dir().join(format!("klf-kira-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create scratch directory");
    let dist = directory.join("dist");
    let klf = Path::new(env!("CARGO_BIN_EXE_klf"));
    run(Command::new(klf).arg("dist").arg("--out").arg(&dist).env_remove("KLF_HOME"));

    let kira = directory.join(klf_target::executable("kira"));
    let definition = root.join("../klf-kira/languages/kira");
    run(Command::new(klf).arg("build").arg(definition).arg("-o").arg(&kira).env("KLF_HOME", &dist));
    let klf_package = dist.join("share/klf/KLF");
    run(Command::new(&kira).arg("test").arg(&klf_package).env("KLF_HOME", &dist));
    // The distributed library is also a normal Kira dependency for tools
    // invoking the compiler directly, without the KLF frontend driver.
    let dependency = format!("KLF={}", klf_package.display());
    run(Command::new(&kira).arg("check").arg(root.join("examples/kira-frontend"))
        .arg("--dependency").arg(&dependency).env("KLF_HOME", &dist));
    let consumer = directory.join(klf_target::executable("klf-consumer"));
    run(Command::new(&kira).arg("build").arg(root.join("examples/kira-klf-consumer"))
        .arg("--dependency").arg(&dependency).args(["--emit", "bin", "-o"])
        .arg(&consumer).env("KLF_HOME", &dist));
    let result = Command::new(&consumer).output().unwrap();
    assert_eq!(result.status.code(), Some(42));
    assert_eq!(result.stdout, b"A-12\n");
    let consumer_bytecode = directory.join("klf-consumer.klbc");
    run(Command::new(&kira).arg("build").arg(root.join("examples/kira-klf-consumer"))
        .arg("--dependency").arg(&dependency).args(["--emit", "klbc", "-o"])
        .arg(&consumer_bytecode).env("KLF_HOME", &dist));
    let vm = dist.join("platforms").join(klf_driver::host_triple()).join(klf_target::executable("klfvm-run"));
    let result = run(Command::new(vm).arg(&consumer_bytecode).args(["--entry", "main"]));
    assert!(String::from_utf8_lossy(&result.stdout).contains("RET 42"));
    let vm = dist.join("platforms").join(klf_driver::host_triple()).join(klf_target::executable("klfvm-run"));
    let result = Command::new(vm).arg(&consumer_bytecode).env("KLF_VM_ENTRY_CONVENTION", "process-exit")
        .output().expect("run KLF consumer as a process");
    assert_eq!(result.status.code(), Some(42));
    assert_eq!(result.stdout, b"A-12\n");

    // A native library loaded by the VM has its own runtime on Windows.
    // Its trap must use the process-exit policy selected by the VM runner.
    if cfg!(target_os = "windows") {
        let trap_bytecode = directory.join("klf-trap.klbc");
        run(Command::new(&kira).arg("build").arg(root.join("examples/kira-klf-trap"))
            .arg("--dependency").arg(&dependency).args(["--emit", "klbc", "-o"])
            .arg(&trap_bytecode).env("KLF_HOME", &dist));
        let vm = dist.join("platforms").join(klf_driver::host_triple()).join(klf_target::executable("klfvm-run"));
        let mut child = Command::new(vm).arg(&trap_bytecode).env("KLF_VM_ENTRY_CONVENTION", "process-exit")
            .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
            .spawn().expect("run KLF trap consumer");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if child.try_wait().expect("poll KLF trap consumer").is_some() { break; }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("KLF trap consumer did not terminate");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let result = child.wait_with_output().expect("collect KLF trap output");
        assert_eq!(result.status.code(), Some(1), "{}", String::from_utf8_lossy(&result.stderr));
        assert!(String::from_utf8_lossy(&result.stderr).contains("KLF-RT-0007"));
        assert!(result.stdout.is_empty());
    }

    let frontend = directory.join(klf_target::executable("mini"));
    let editor = directory.join("editor");
    run(Command::new(klf).arg("editor").arg("gen").arg(root.join("examples/kira-frontend"))
        .args(["--editor", "helix", "--out"]).arg(&editor).env("KLF_HOME", &dist).env("KLF_KIRA", &kira));
    let languages = std::fs::read_to_string(editor.join("helix/languages.toml")).unwrap();
    assert!(languages.contains("source.mini") && languages.contains("mini.manifest"));
    let grammar = std::fs::read_to_string(editor.join("textmate/mini.tmLanguage.json")).unwrap();
    assert!(grammar.contains("main"));
    run(Command::new(klf).arg("build").arg(root.join("examples/kira-frontend"))
        .arg("-o").arg(&frontend).env("KLF_HOME", &dist).env("KLF_KIRA", &kira));
    let unit = run(Command::new(klf).arg("test").arg(root.join("examples/kira-frontend"))
        .arg("--unit").env("KLF_HOME", &dist).env("KLF_KIRA", &kira).env("KLF_TEST_EDITOR_FACTS", "1"));
    assert!(String::from_utf8_lossy(&unit.stdout).contains("PASS sharedFrontendTestingUsesTheLanguageExtension"));
    let facts_input = directory.join("facts.mini");
    std::fs::write(&facts_input, "main 40").unwrap();
    let mut analysis = Command::new(&frontend).arg("lsp-worker")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
        .env("KLF_HOME", &dist).env("KLF_TEST_EDITOR_FACTS", "1").spawn().expect("start analysis worker");
    let argument = facts_input.to_string_lossy().replace('\\', "/");
    write!(analysis.stdin.take().unwrap(), "argument\t{argument}\n").unwrap();
    let analysis = analysis.wait_with_output().expect("wait for analysis worker");
    assert!(analysis.status.success());
    let facts = String::from_utf8(analysis.stdout).expect("UTF-8 analysis");
    for text in ["symbol hé", "type hé", "value hé", "hint hé", "link hé", "run hé"] {
        assert!(facts.contains(text), "missing {text} in {facts}");
    }
    let mut project = Command::new(&frontend).arg("lsp-project-worker")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
        .env("KLF_HOME", &dist).spawn().expect("start project worker");
    project.stdin.take().unwrap().write_all(b"__mini_project_test__").unwrap();
    let project = project.wait_with_output().expect("wait for project worker");
    assert!(project.status.success());
    let project: serde_json::Value = serde_json::from_slice(&project.stdout).expect("project JSON");
    assert_eq!(project["project"]["name"], "mini");
    assert_eq!(project["project"]["arguments"][0], "input.mini");
    assert_eq!(project["project"]["watch"][0], "mini.manifest");
    assert_eq!(project["failed"], false);
    let mut process = Command::new(&frontend).arg("lsp-project-worker")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
        .env("KLF_HOME", &dist).env("KLF_TEST_CHILD", &frontend).spawn().expect("start process project worker");
    process.stdin.take().unwrap().write_all(b"__mini_process_test__").unwrap();
    let process = process.wait_with_output().expect("wait for process project worker");
    assert!(process.status.success());
    let process: serde_json::Value = serde_json::from_slice(&process.stdout).expect("process project JSON");
    assert_eq!(process["project"]["name"], "process");
    assert_eq!(process["failed"], false);
    let mut failed_project = Command::new(&frontend).arg("lsp-project-worker")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
        .env("KLF_HOME", &dist).spawn().expect("start project worker");
    failed_project.stdin.take().unwrap().write_all(b"__mini_project_error__").unwrap();
    let failed_project = failed_project.wait_with_output().expect("wait for project worker");
    assert!(failed_project.status.success());
    let error: serde_json::Value = serde_json::from_slice(&failed_project.stdout).expect("project error JSON");
    assert_eq!(error["failed"], true);
    assert_eq!(error["diagnostics"][0]["code"], 43);
    assert_eq!(error["diagnostics"][0]["severity"], "warning");
    assert_eq!(error["diagnostics"][1]["code"], 42);
    assert_eq!(error["diagnostics"][1]["severity"], "error");
    assert_eq!(error["diagnostics"][1]["start"], 9);
    assert_eq!(error["diagnostics"][1]["end"], 13);
    assert_eq!(error["diagnostics"][1]["message"], "Invalid project: The project manifest is invalid");
    std::fs::write(directory.join("project.manifest"), "first line\ninvalid manifest\n").unwrap();
    let mut manifest_error = Command::new(&frontend).arg("lsp-project-worker")
        .current_dir(&directory).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
        .env("KLF_HOME", &dist).spawn().expect("start manifest project worker");
    manifest_error.stdin.take().unwrap().write_all(b"__mini_manifest_error__").unwrap();
    let manifest_error = manifest_error.wait_with_output().unwrap();
    assert!(manifest_error.status.success());
    let error: serde_json::Value = serde_json::from_slice(&manifest_error.stdout).unwrap();
    assert_eq!(Path::new(error["diagnostics"][0]["path"].as_str().unwrap()), directory.join("project.manifest"));
    assert_eq!(error["diagnostics"][0]["start"], 11);
    assert_eq!(error["diagnostics"][0]["end"], 18);
    let mut warning = Command::new(&frontend).arg("lsp-project-worker")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
        .env("KLF_HOME", &dist).spawn().expect("start warning project worker");
    warning.stdin.take().unwrap().write_all(b"__mini_project_warning__").unwrap();
    let warning = warning.wait_with_output().unwrap();
    assert!(warning.status.success());
    let warning: serde_json::Value = serde_json::from_slice(&warning.stdout).unwrap();
    assert_eq!(warning["failed"], false);
    assert_eq!(warning["diagnostics"][0]["severity"], "warning");
    assert_eq!(warning["diagnostics"][0]["code"], 43);
    let mut default_project = Command::new(&frontend).arg("lsp-project-worker")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
        .env("KLF_HOME", &dist).spawn().expect("start project worker");
    default_project.stdin.take().unwrap().write_all(b"ordinary.mini").unwrap();
    let default_project = default_project.wait_with_output().expect("wait for project worker");
    assert!(default_project.status.success());
    let default_project: serde_json::Value = serde_json::from_slice(&default_project.stdout).unwrap();
    assert_eq!(default_project["project"], serde_json::Value::Null);
    assert_eq!(default_project["failed"], false);
    let input = directory.join("main-é.mini");
    std::fs::write(&input, "main 17").expect("write program");
    run(Command::new(&frontend).arg("format").arg(&input).env("KLF_HOME", &dist));
    run(Command::new(&frontend).arg("format").arg(&input).arg("--check").env("KLF_HOME", &dist));
    assert_eq!(std::fs::read_to_string(&input).expect("formatted input"), "main 17\n");
    std::fs::write(&input, "main 17").unwrap();
    let check_format = Command::new(klf).arg("format").arg(root.join("examples/kira-frontend"))
        .arg(&input).arg("--check").env("KLF_HOME", &dist).env("KLF_KIRA", &kira)
        .output().expect("check Kira frontend format");
    assert!(!check_format.status.success());
    assert_eq!(std::fs::read_to_string(&input).unwrap(), "main 17");
    run(Command::new(klf).arg("format").arg(root.join("examples/kira-frontend"))
        .arg(&input).env("KLF_HOME", &dist).env("KLF_KIRA", &kira));
    assert_eq!(std::fs::read_to_string(&input).unwrap(), "main 17\n");
    let mut formatting = Command::new(klf).arg("format").arg(root.join("examples/kira-frontend"))
        .env("KLF_HOME", &dist).env("KLF_KIRA", &kira)
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
        .spawn().expect("start Kira frontend stdin formatter");
    formatting.stdin.take().unwrap().write_all(b"main 19").unwrap();
    let formatting = formatting.wait_with_output().unwrap();
    assert!(formatting.status.success());
    assert_eq!(formatting.stdout, b"main 19\n");
    let native = directory.join(klf_target::executable("program"));
    run(Command::new(&frontend).arg(&input).args(["--emit", "bin", "-o"]).arg(&native).env("KLF_HOME", &dist));
    let result = run(&mut Command::new(&native));
    assert!(String::from_utf8_lossy(&result.stdout).contains("RET 19"));

    let result = run(Command::new(&frontend).arg(&input).args(["--emit", "run"]).env("KLF_HOME", &dist));
    assert!(String::from_utf8_lossy(&result.stdout).contains("RET 19"));

    let bytecode = directory.join("program.klbc");
    run(Command::new(&frontend).arg(&input).args(["--mini-bytecode", "-o"]).arg(&bytecode).env("KLF_HOME", &dist));
    let runner = dist.join("platforms").join(klf_driver::host_triple()).join(klf_target::executable("klfvm-run"));
    let result = run(Command::new(runner).arg(&bytecode).args(["--entry", "main"]));
    assert!(String::from_utf8_lossy(&result.stdout).contains("RET 19"));

    let shader = directory.join(klf_target::executable("shader"));
    run(Command::new(klf).arg("build").arg(root.join("examples/kira-shader"))
        .arg("--language").arg(root.join("examples/kira-shader-hlsl"))
        .arg("--language").arg(root.join("examples/kira-shader/test-out.kcore"))
        .arg("-o").arg(&shader).env("KLF_HOME", &dist).env("KLF_KIRA", &kira));
    let shader_input = directory.join("example.tshader");
    std::fs::write(&shader_input, "shader").expect("write shader input");
    let result = run(Command::new(&shader).arg("build").arg(&shader_input)
        .args(["--target", "wgsl", "--emit", "source"]).env("KLF_HOME", &dist));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "// emitted by Kira\n");
    let result = run(Command::new(&shader).arg("build").arg(&shader_input)
        .args(["--target", "hlsl", "--emit", "source"]).env("KLF_HOME", &dist));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "// emitted by HLSL Kira\n");
    let result = run(Command::new(&shader).arg("build").arg(&shader_input)
        .args(["--target", "msl", "--emit", "source"]).env("KLF_HOME", &dist));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "// emitted by KCore\n");
    let kcore_shader = directory.join(klf_target::executable("kcore-shader"));
    run(Command::new(klf).arg("build").arg(root.join("examples/kira-shader/test-out.kcore"))
        .args(["--emit", "frontend"])
        .arg("--language").arg(root.join("examples/kira-shader"))
        .arg("--language").arg(root.join("examples/kira-shader-hlsl"))
        .arg("-o").arg(&kcore_shader).env("KLF_HOME", &dist).env("KLF_KIRA", &kira));
    for (target, expected) in [
        ("wgsl", "// emitted by Kira\n"),
        ("hlsl", "// emitted by HLSL Kira\n"),
        ("msl", "// emitted by KCore\n"),
    ] {
        let result = run(Command::new(&kcore_shader).arg("build").arg(&shader_input)
            .args(["--target", target, "--emit", "source"]).env("KLF_HOME", &dist));
        assert_eq!(String::from_utf8_lossy(&result.stdout), expected);
    }
    let kcore_out = root.join("examples/kira-shader/test-out.kcore");
    let duplicate_out = directory.join("duplicate-out.kcore");
    std::fs::copy(&kcore_out, &duplicate_out).unwrap();
    for (languages, expected) in [
        (vec![root.join("definitions/mini")], "is not a shader language"),
        (vec![duplicate_out.clone(), duplicate_out.clone()], "already linked"),
    ] {
        let mut command = Command::new(klf);
        command.arg("build").arg(&kcore_out).args(["--emit", "frontend"]);
        for language in languages { command.arg("--language").arg(language); }
        let result = command.arg("-o").arg(directory.join("invalid-shader"))
            .env("KLF_HOME", &dist).output().expect("start invalid shader build");
        assert!(!result.status.success(), "invalid shader build succeeded");
        assert!(String::from_utf8_lossy(&result.stderr).contains(expected), "{}", String::from_utf8_lossy(&result.stderr));
    }
    let karc = root.join("../karc");
    let karc_tests = run(Command::new(klf).arg("test").arg(&karc)
        .env("KLF_HOME", &dist).env("KLF_KIRA", &kira));
    let karc_tests = String::from_utf8_lossy(&karc_tests.stdout);
    for name in [
        "compilesArrayParameters",
        "comprehensiveLibcRunsOnTheNativeBackend",
        "callsAFunctionDefinedInAnotherUnit",
        "Programs::standard-library-filesystem-vm",
        "Programs::std-strings-allocation-classes-and-printf",
        "KArcEditor::format C function",
    ] {
        assert!(karc_tests.contains(&format!("PASS {name}")), "missing KArc test {name}: {karc_tests}");
    }
    let karc_editor = directory.join("karc-editor");
    run(Command::new(klf).arg("editor").arg("gen").arg(&karc)
        .args(["--editor", "helix", "--out"]).arg(&karc_editor).env("KLF_HOME", &dist).env("KLF_KIRA", &kira));
    let grammar = std::fs::read_to_string(karc_editor.join("textmate/karc.tmLanguage.json")).unwrap();
    assert!(grammar.contains("source.karc") && grammar.contains("typedef"));
    let languages = std::fs::read_to_string(karc_editor.join("helix/languages.toml")).unwrap();
    assert!(languages.contains("source.karc") && languages.contains("compile_commands.json"));
    let sysroot = directory.join("karc-sysroot");
    run(Command::new(klf).arg("build").arg(&karc).arg("--out").arg(&sysroot)
        .env("KLF_HOME", &dist).env("KLF_KIRA", &kira));
    let lock = std::fs::read_to_string(sysroot.join("definition.lock")).unwrap();
    assert!(lock.contains("app/Language.kira") && lock.contains("package.kira"));
    assert!(sysroot.join("include/karc/cstdlib").is_file());
    assert!(sysroot.join("lib/karc").join(klf_driver::host_triple()).join("libstd.a").is_file());
    assert!(sysroot.join("lib/karc/bytecode/64/std.klbc").is_file());
    let karc_driver = sysroot.join("bin").join(klf_target::executable("karc"));
    for extension in ["c", "cpp", "krc", "h", "krch"] {
        let file = directory.join(format!("format.{extension}"));
        std::fs::write(&file, "int f(){return 42;}\n").unwrap();
        run(Command::new(&karc_driver).arg("format").arg(&file).env("KLF_HOME", &dist));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "int f() {\n    return 42;\n}\n");
        run(Command::new(&karc_driver).arg("format").arg(&file).arg("--check").env("KLF_HOME", &dist));
    }
    let source = karc.join("corpus/standard-library-allocation.cpp");
    let program = directory.join(klf_target::executable("karc-program"));
    run(Command::new(&karc_driver).arg(&source).args(["--emit", "bin", "-o"]).arg(&program)
        .current_dir(&directory).env("KLF_HOME", &dist));
    assert_eq!(Command::new(&program).status().unwrap().code(), Some(42));
    let bytecode = directory.join("karc-program.klbc");
    run(Command::new(&karc_driver).arg(&source).args(["--emit", "klbc", "-o"]).arg(&bytecode)
        .current_dir(&directory).env("KLF_HOME", &dist));
    let runner = sysroot.join("bin").join(klf_target::executable("karc-run"));
    assert_eq!(Command::new(runner).arg(&bytecode).status().unwrap().code(), Some(42));
    std::fs::remove_dir_all(directory).expect("remove scratch directory");
}
