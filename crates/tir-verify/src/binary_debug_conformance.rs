use tir_diag::Severity;
use tir_format::{
    BinaryCodeTable, BinaryDebugTable, BinaryModule, BinaryModuleBuilder, DebugCompileUnit,
    DebugFileRecord, DebugLocation, DebugScope, DebugScopeKind, DebugSourceLocation,
    DebugSourceType, Section, assemble_text_module, parse_text_module,
};

use super::verify_binary_module_records;

fn fixture() -> BinaryModuleBuilder {
    let source = "module @Debug tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Debug.run() -> unit engine(native) {\nbb0:\n  return\n}\n";
    let parsed = parse_text_module(source).expect("DEBUG source parses");
    let raw = assemble_text_module(&parsed).expect("fixture encodes");
    let module = BinaryModule::decode(&raw).expect("module decodes");
    let types = module.type_table(u64::MAX).expect("types");
    let signatures = module
        .signature_table(u64::MAX, &types)
        .expect("signatures");
    let constants = module.constant_table(u64::MAX, &types).expect("constants");
    let globals = module
        .global_table(u64::MAX, &types, &constants)
        .expect("globals");
    let code: BinaryCodeTable = module.code_table(u64::MAX, &signatures).expect("code");
    let mut builder = BinaryModuleBuilder::new(
        module.header,
        module.module_record(u64::MAX).expect("module record"),
    );
    builder.types = types;
    builder.signatures = signatures;
    builder.constants = constants;
    builder.globals = globals;
    builder.code = code;
    builder.analysis = module.analysis_summary(u64::MAX).expect("analysis");
    builder.debug = BinaryDebugTable {
        files: vec![DebugFileRecord {
            path: "src/main.kira".into(),
            content_hash: None,
            generated_text: None,
        }],
        compile_units: vec![DebugCompileUnit {
            language: "kira".into(),
            producer: "klf".into(),
        }],
        source_types: vec![DebugSourceType::Base {
            name: "i32".into(),
            encoding: 5,
            bit_width: 32,
        }],
        scopes: vec![DebugScope {
            kind: DebugScopeKind::Function,
            function_index: 0,
            compile_unit: 0,
            parent: None,
        }],
        locations: vec![DebugLocation {
            function_index: 0,
            instruction_ordinal: 0,
            source: DebugSourceLocation {
                file_index: 0,
                line: 3,
                column: 1,
            },
            scope: Some(0),
        }],
        ..BinaryDebugTable::default()
    };
    builder
}

fn errors(module: &BinaryModule) -> Vec<String> {
    verify_binary_module_records(module, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .map(|issue| issue.code.to_string())
        .collect()
}

#[test]
fn debug_section_is_verified_against_real_code() {
    let builder = fixture();
    let module = BinaryModule::decode(&builder.encode().expect("DEBUG module encodes"))
        .expect("DEBUG module decodes");
    assert!(
        errors(&module).is_empty(),
        "valid DEBUG records are rejected"
    );
}

#[test]
fn debug_out_of_range_function_is_a_v2_error() {
    let mut builder = fixture();
    let mut module = BinaryModule::decode(&builder.encode().expect("DEBUG module encodes"))
        .expect("DEBUG module decodes");
    builder.debug.scopes[0].function_index = 99;
    builder.debug.locations[0].function_index = 99;
    let strings = module.string_table(u64::MAX).expect("STRINGS");
    let invalid = builder
        .debug
        .encode(&strings)
        .expect("self-consistent DEBUG encoding");
    let section = module
        .sections
        .iter_mut()
        .find(|section| section.kind == 17)
        .expect("DEBUG section exists");
    *section = Section::from_uncompressed(17, invalid);
    let actual = errors(&module);
    assert!(
        actual.iter().any(|code| code == "TIR-VFY-0002"),
        "invalid function reference must be V2: {actual:?}"
    );
}
