//! A bytecode native library on Windows owns a separate runtime archive.

#![cfg(all(windows, target_env = "msvc"))]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn link(clang: &Path, source: &Path, output: &Path, runtime: &Path, flags: &[&str]) {
    let result = Command::new(clang)
        .args(flags)
        .arg(source)
        .arg(runtime)
        .args(klf_target::MSVC_SYSTEM_LIBRARIES)
        .arg("-o")
        .arg(output)
        .output()
        .expect("start clang");
    assert!(result.status.success(), "{}\n{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
}

#[test]
fn attached_runtime_uses_host_process_trap_policy() {
    let clang = klf_target::llvm_tool("clang").expect("bundled clang");
    let built = Command::new(env!("CARGO"))
        .args(["build", "-p", "klf-native-rt"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .status()
        .expect("build native archive");
    assert!(built.success());
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        std::path::PathBuf::from,
    );
    let runtime = target.join("debug").join(klf_target::static_library("klf_native_rt", klf_target::host_triple()));
    let directory = std::env::temp_dir().join(format!("klf-process-host-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create scratch directory");
    let dll_source = directory.join("library.c");
    let host_source = directory.join("host.c");
    let dll = directory.join("library.dll");
    let host = directory.join("host.exe");
    std::fs::write(&dll_source, "extern int klfrt_process_mode(void);\nextern void klfrt_trap(int);\n__declspec(dllexport) int process_mode(void) { return klfrt_process_mode(); }\n__declspec(dllexport) void run_trap(void) { klfrt_trap(7); }\n").unwrap();
    std::fs::write(&host_source, "__declspec(dllimport) void * __stdcall LoadLibraryA(const char *);\n__declspec(dllimport) void * __stdcall GetProcAddress(void *, const char *);\nextern void klfrt_process_start(int);\nint main(int argc, char **argv) { if (argc != 2) return 3; void *lib = LoadLibraryA(argv[1]); if (!lib) return 4; int (*mode)(void) = (int (*)(void))GetProcAddress(lib, \"process_mode\"); if (!mode) return 5; if (mode() != 0) return 6; klfrt_process_start(1); if (mode() != 3) return 7; void (*trap)(void) = (void (*)(void))GetProcAddress(lib, \"run_trap\"); if (!trap) return 8; trap(); return 9; }\n").unwrap();
    link(&clang, &dll_source, &dll, &runtime, &["-shared", "-fuse-ld=lld"]);
    link(&clang, &host_source, &host, &runtime, &["-fuse-ld=lld", "-Wl,/INCLUDE:klfrt_process_active", "-Wl,/EXPORT:klfrt_process_active"]);
    let mut child = Command::new(host).arg(dll)
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .spawn().expect("run host");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().expect("poll host").is_some() { break; }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("attached runtime trap did not terminate the host");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let result = child.wait_with_output().expect("collect host output");
    assert_eq!(result.status.code(), Some(1), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stderr).contains("KLF-RT-0007"));
    assert!(result.stdout.is_empty());
}
