use super::*;
use klf_target::Target;

fn session() -> Session {
    let target = "aarch64-apple-darwin"
        .parse::<Target>()
        .expect("valid target");
    Session::new(target)
}

/// The output, text, math, number, file, network, cryptography, process,
/// time, host and runtime families.
const PROGRAM_SERVICES: &[&str] = &[
    "klf.thread.",
    "klf.io.",
    "klf.text.",
    "klf.math.",
    "klf.number",
    "klf.fs.",
    "klf.net.",
    "klf.crypto.",
    "klf.process.",
    "klf.time.",
    "klf.host.",
    "klf.runtime.",
];

/// Every manifest service a definition can name has to reach the prelude, or a
/// call to it fails name resolution with no way to tell why. The `units` phase
/// ABI services are the exception: the lowerer emits them around the declared
/// signature, so a definition never names them. The `klf.driver.*` and
/// `klf.cli.*` services are the same case for the generated `compatDriver` and
/// command entries, and the `klf.number.*` pair is the same case for the
/// `Number` type, whose crossing the lowerer emits from the type itself. The
/// [`PROGRAM_SERVICES`] serve the programs a frontend compiles, which declare
/// them themselves, not the definition.
#[test]
fn every_manifest_service_gets_a_prelude_wrapper() {
    let session = session();
    let signatures = signatures(&session);
    let mut missing = Vec::new();
    for entry in session.kernel_services().services() {
        let Some(name) = wrapper_name(&entry.name) else {
            missing.push(entry.name.clone());
            continue;
        };
        let (_, signature) = signatures
            .iter()
            .find(|(candidate, _)| candidate.as_str() == name)
            .unwrap_or_else(|| panic!("{name} is in the prelude"));
        // A `String` crosses as its bytes and their length.
        let words: usize = signature
            .params
            .iter()
            .map(|param| match param {
                FeType::Struct(name) if name.as_str() == "String" => 2,
                _ => 1,
            })
            .sum();
        assert_eq!(
            words,
            entry.signature.parameters.len(),
            "wrapper {name} does not match the manifest arity of {}",
            entry.name
        );
    }
    missing.retain(|name| {
        !PROGRAM_SERVICES
            .iter()
            .any(|family| name.starts_with(family))
    });
    assert_eq!(
        missing,
        [
            "klf.editor.project_field",
            "klf.driver.argument_len",
            "klf.driver.argument_copy",
            "klf.driver.request_push",
            "klf.cli.count",
            "klf.cli.int",
            "klf.cli.string_len",
            "klf.cli.string_copy",
            "klf.definition.units_push",
            "klf.definition.units_finish",
            "klf.source.ffi_header",
            "klf.source.ridl_module",
            "klf.shader.read.text_len",
            "klf.shader.read.text_copy",
        ]
    );
}
