use super::*;

pub(super) fn build(
    metadata: &FrontendMetadata,
    invocation: arguments::Invocation,
    driver_arguments: &[String],
) -> Result<i32, Failure> {
    let triple = invocation
        .target
        .clone()
        .unwrap_or_else(|| metadata.target.clone());
    let test_compile_only =
        std::env::var_os("KLF_TEST_COMPILE_ONLY").as_deref() == Some(std::ffi::OsStr::new("1"));
    if !test_compile_only && matches!(invocation.emit.as_str(), "bin" | "lib" | "shared" | "run") {
        toolchain::check_target(&triple).map_err(Failure::Driver)?;
    }
    // The same status surface `klf` draws, from the same crate: a driver is a
    // client CLI too, and its phases are the ones the definition reports.
    let mut display =
        klf_diag::status::begin(&format!("Building {}", metadata.driver), invocation.timings);
    // A run and a linked binary are reused whole while every input and linked
    // archive still holds what the previous build read. Check this before
    // loading source files or initializing the phase runner.
    let reuses = matches!(invocation.emit.as_str(), "run" | "bin");
    if reuses {
        klf_services::record_accesses();
    }
    let mut session = new_session(metadata, &invocation, &triple).map_err(Failure::Driver)?;
    let mut cache =
        module_cache::DiskModuleCache::open(metadata, &invocation, &triple, session.std_root());
    let cached_output = match invocation.emit.as_str() {
        "run" => {
            Some(run::executable_path(invocation.output.clone(), &invocation.inputs, &triple).0)
        }
        "bin" => Some(binary_output(&invocation)?),
        _ => None,
    };
    let program_key = cache.as_ref().filter(|_| reuses).map(|cache| {
        let mut key = cache.build_key();
        for input in &invocation.inputs {
            let input = std::path::absolute(input).unwrap_or_else(|_| input.clone());
            key.push_str(&format!("input {}\n", input.display()));
        }
        if let Some(output) = &cached_output {
            let output = std::path::absolute(output).unwrap_or_else(|_| output.clone());
            key.push_str(&format!("output {}\n", output.display()));
        }
        key.push_str(&format!(
            "system linker {}\n",
            std::env::var("KLF_USE_SYSTEM_LINKER").unwrap_or_default()
        ));
        if let Some(executable) = std::env::current_exe()
            .ok()
            .and_then(|path| path.metadata().ok())
        {
            let modified = executable
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |modified| modified.as_nanos());
            key.push_str(&format!("driver {} {modified}\n", executable.len()));
        }
        key
    });
    if let (Some(key), Some(output)) = (&program_key, &cached_output) {
        if let Some(warnings) = program_cache::reusable(output, key) {
            if !warnings.is_empty() {
                klf_diag::raw_err!("{warnings}");
            }
            display.end();
            klf_diag::out!("Built: Ok {}", output.display());
            if invocation.emit == "run" {
                return run::run(
                    metadata.declares(klf_defsvc::Hook::RunProgram),
                    output,
                    &invocation.program_arguments,
                    driver_arguments,
                )
                .map_err(Failure::Driver);
            }
            return Ok(0);
        }
    }
    let files = add_inputs(&mut session, metadata, &invocation).map_err(Failure::Driver)?;
    let mut make_runner = || Ok(NativePhases::new());
    let driven = match &mut cache {
        Some(cache) => drive_cached(&mut session, &files, &mut make_runner, cache),
        None => drive(&mut session, &files, &mut make_runner),
    };
    let module = driven.map_err(|error| match error {
        // A phase failure is the definition rejecting the program: the message
        // is the diagnostic it reported, rendered against the source.
        DriveError::Phase { message, .. } => Failure::Rejected(message),
        // Two units disagreeing is the compiler's fault, said as a diagnostic
        // of its own with what each unit holds.
        DriveError::Conflict(message) => Failure::Rejected(format!(
            "{}: error[KLF-KIR-9002]: {message}",
            metadata.driver
        )),
        other => Failure::Driver(other.to_string()),
    })?;
    let warnings = cache.as_ref().map_or_else(
        || render_kept_diagnostics(&session),
        |cache| cache.kept_diagnostics(&session),
    );
    if !warnings.is_empty() {
        klf_diag::raw_err!("{warnings}");
    }
    let level = optimization_level(&session);
    if invocation.emit == "check" || test_compile_only {
        display.end();
        klf_diag::out!("Check: Ok");
        return Ok(0);
    }
    if let Some(device) = &invocation.device {
        display.end();
        return push::push(metadata, &module, device).map_err(Failure::Driver);
    }

    if invocation.emit == "run" {
        let (executable, temporary) =
            run::executable_path(invocation.output.clone(), &invocation.inputs, &triple);
        run::prepare(&executable).map_err(Failure::Driver)?;
        let links = program_key
            .as_ref()
            .and_then(|_| output::linked_archives(&module, &metadata.name, &triple));
        output::emit_binary(module, &executable, &metadata.name, metadata.entry, level)
            .map_err(Failure::Driver)?;
        if let (Some(key), Some(links)) = (&program_key, &links) {
            program_cache::keep(&executable, key, links, &warnings);
        }
        display.end();
        klf_diag::out!("Built: Ok {}", executable.display());
        let status = run::run(
            metadata.declares(klf_defsvc::Hook::RunProgram),
            &executable,
            &invocation.program_arguments,
            driver_arguments,
        );
        if temporary {
            run::remove(&executable);
        }
        return status.map_err(Failure::Driver);
    }
    let output = match invocation.output.clone() {
        Some(output) => output,
        None if invocation.emit == "bin" => binary_output(&invocation)?,
        None => {
            let mut path = invocation.inputs[0].clone();
            path.set_extension(match invocation.emit.as_str() {
                "klbc" => "klbc",
                "lib" => "a",
                "ridl" => "ridl",
                "shared" if toolchain::is_apple(&triple) => "dylib",
                "shared" => "so",
                _ => "kir",
            });
            path
        }
    };
    if invocation.emit == "bin" {
        let links = program_key
            .as_ref()
            .and_then(|_| output::linked_archives(&module, &metadata.name, &triple));
        output::emit_binary(module, &output, &metadata.name, metadata.entry, level)
            .map_err(Failure::Driver)?;
        if let (Some(key), Some(links)) = (&program_key, &links) {
            program_cache::keep(&output, key, links, &warnings);
        }
        display.end();
        klf_diag::out!("Built: Ok {}", output.display());
        return Ok(0);
    }
    if invocation.emit == "lib" {
        output::emit_library(
            &module,
            &output,
            &metadata.name,
            invocation.export.as_deref(),
        )
        .map_err(Failure::Driver)?;
        display.end();
        klf_diag::out!("Built: Ok {}", output.display());
        return Ok(0);
    }
    if invocation.emit == "ridl" {
        let written = write_contract(
            &module,
            &metadata.name,
            &session,
            &output,
            invocation.ridl_source,
        );
        if written.is_ok() {
            display.end();
            klf_diag::out!("Built: Ok {}", output.display());
        }
        return written.map(|()| 0);
    }
    if invocation.emit == "shared" {
        if let Some(contract) = &invocation.contract {
            write_contract(
                &module,
                &metadata.name,
                &session,
                contract,
                invocation.ridl_source,
            )?;
        }
        output::emit_shared(&module, &output, &metadata.name).map_err(Failure::Driver)?;
        display.end();
        klf_diag::out!("Built: Ok {}", output.display());
        return Ok(0);
    }
    let bytes = if invocation.emit == "klbc" {
        klf_diag::progress::report("lower bytecode");
        let mut program = klf_vm::lower_module(&module)
            .map_err(|error| format!("cannot lower KIR to bytecode: {error}"))
            .map_err(Failure::Driver)?;
        output::link_std_bytecode(&mut program, &module, &metadata.name)
            .map_err(Failure::Driver)?;
        vm_natives::emit_natives(&module, &mut program, &output).map_err(Failure::Driver)?;
        klf_vm::encode(&program)
    } else if level == klf_kir_passes::Level::O0 {
        module.to_string().into_bytes()
    } else {
        output::machine_module(module, level, &output)
            .map_err(Failure::Driver)?
            .to_string()
            .into_bytes()
    };
    let written = std::fs::write(&output, bytes)
        .map_err(|error| format!("cannot write {}: {error}", output.display()))
        .map_err(Failure::Driver);
    if written.is_ok() {
        display.end();
        klf_diag::out!("Built: Ok {}", output.display());
    }
    written.map(|()| 0)
}
