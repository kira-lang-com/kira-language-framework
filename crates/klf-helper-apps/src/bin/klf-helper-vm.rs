//! Runtime-selected VM-only Kira helper. The host passes compiled images,
//! not a command supplied by application code. RIDL imports are resolved
//! against the private inherited host connection before initialization.

#![cfg(unix)]

use klf_helper_apps::{attached_connection, bind_generated_ridl_imports};
use klf_vm::{Capabilities, Instance, Outcome};
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let image = PathBuf::from(args.next().ok_or("missing Kira VM image")?);
    let kir = PathBuf::from(args.next().ok_or("missing Kira generated import map")?);
    if args.next().is_some() {
        return Err("unexpected VM helper launch arguments".into());
    }
    let connection = Arc::new(attached_connection()?.ok_or("not launched by a Kira host")?);
    let program = klf_vm::decode(&std::fs::read(image)?)
        .map_err(|error| format!("cannot decode Kira VM image: {error}"))?;
    let module = klf_kir::kirb::decode(&std::fs::read(kir)?)
        .ok_or("cannot decode Kira generated RIDL metadata")?;
    let mut capabilities = Capabilities::new();
    bind_generated_ridl_imports(&module, &program, &connection, &mut capabilities)?;
    let vm = Instance::load_restricted(program, capabilities)?;
    let outcome = match vm.initialize() {
        Outcome::Return(_) => {
            let entry = vm.function_index("main").ok_or("Kira helper has no main")?;
            vm.run(entry, &[])
        }
        other => other,
    };
    connection.close()?;
    match outcome {
        Outcome::Return(_) => Ok(()),
        Outcome::Fail(value) => Err(format!("Kira helper failed: {value}").into()),
        Outcome::Trap(code) => Err(format!("Kira helper trapped: {code}").into()),
    }
}
