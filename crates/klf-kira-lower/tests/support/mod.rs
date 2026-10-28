//! Shared harness: lower a KLF Kira source file and run `entry` on the VM.

use klf_kir_passes::monomorphize;
use klf_kira_lower::lower_file;
use klf_kira_syntax::parse;
use klf_services::Session;
use klf_vm::{Outcome, run_module};

pub fn run(source: &str, entry: &str, args: &[u64]) -> Outcome {
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    let mut session = Session::new(target);
    let file_id = session.add_file("t.klfd", source);
    let parsed = parse(session.file(file_id));
    assert!(
        parsed.diagnostics.is_empty(),
        "parse errors: {:?}",
        parsed.diagnostics
    );
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    let module = lower_file(&mut session, "Test", target, &parsed.file);
    let module = module.unwrap_or_else(|| panic!("lowering errors: {:?}", session.diagnostics()));
    let module = monomorphize(&module);
    run_module(&module, entry, args).expect("module runs")
}
