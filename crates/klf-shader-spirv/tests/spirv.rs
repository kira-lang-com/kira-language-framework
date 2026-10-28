//! The SPIR-V backend on the shared fixtures. With `KLF_SPIRV_VAL` naming
//! a `spirv-val` binary, each module is also validated for Vulkan 1.1.

use klf_shader::{Stage, Target};
use klf_shader_passes::PassOptions;
use klf_shader_valid::{Options, validate};

fn units(source: &str) -> Vec<(Stage, Vec<u32>)> {
    units_with(source, &PassOptions::default(), false)
}

fn units_with(source: &str, options: &PassOptions, debug: bool) -> Vec<(Stage, Vec<u32>)> {
    let mut module = klf_shader::parse::parse(source).expect("the fixture parses");
    klf_shader_passes::normalize(&mut module);
    let validation = validate(&module, Options { wgsl: false });
    assert!(validation.is_valid(), "{:?}", validation.problems);
    let assignment = klf_shader_bind::assign(&module, 0).expect("bindings are assigned");
    let (units, _) =
        klf_shader_passes::run(&module, Target::Spirv, options).expect("the passes run");
    units
        .iter()
        .map(|unit| {
            let validation = validate(&unit.module, Options { wgsl: false });
            assert!(validation.is_valid(), "{:?}", validation.problems);
            let info = validation.info.expect("info");
            let resources = assignment.for_unit(&module, &unit.module);
            let words = klf_shader_spirv::write(&unit.module, &info, &resources, debug)
                .unwrap_or_else(|problems| panic!("{problems:?}"));
            (unit.module.entry_points[0].stage, words)
        })
        .collect()
}

fn check_with_spirv_val(name: &str, units: &[(Stage, Vec<u32>)]) {
    let Ok(validator) = std::env::var("KLF_SPIRV_VAL") else {
        return;
    };
    let directory = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("spirv");
    std::fs::create_dir_all(&directory).expect("the output directory");
    for (stage, words) in units {
        let path = directory.join(format!("{name}.{}.spv", stage.suffix()));
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        std::fs::write(&path, bytes).expect("the module is written");
        let output = std::process::Command::new(&validator)
            .args(["--target-env", "vulkan1.1"])
            .arg(&path)
            .output()
            .expect("spirv-val runs");
        assert!(
            output.status.success(),
            "spirv-val rejects {}:\n{}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn the_section_18_example_is_one_module_per_stage() {
    let units = units(include_str!(
        "../../klf-shader-valid/tests/fixtures/tri.ksir"
    ));
    assert_eq!(units.len(), 2);
    for (_, words) in &units {
        assert_eq!(words[0], 0x0723_0203);
        assert_eq!(words[1], 0x0001_0300);
    }
    assert!(klf_shader_spirv::hex(&units[0].1).starts_with("0723020300010300"));
    check_with_spirv_val("tri", &units);
}

#[test]
fn sampling_switches_and_loops_are_structured() {
    let units = units(include_str!(
        "../../klf-shader-valid/tests/fixtures/sprite.ksir"
    ));
    assert_eq!(units.len(), 2);
    check_with_spirv_val("sprite", &units);
}

#[test]
fn storage_buffers_atomics_and_barriers_are_written() {
    let units = units(include_str!(
        "../../klf-shader-valid/tests/fixtures/scan.ksir"
    ));
    assert_eq!(units.len(), 1);
    check_with_spirv_val("scan", &units);
}

#[test]
fn buffer_aggregates_are_copied_between_layouts() {
    let units = units(include_str!(
        "../../klf-shader-valid/tests/fixtures/lit.ksir"
    ));
    assert_eq!(units.len(), 2);
    check_with_spirv_val("lit", &units);
}

/// Each instruction after the header: its opcode and operands.
fn instructions(words: &[u32]) -> Vec<(u32, &[u32])> {
    let mut found = Vec::new();
    let mut at = 5;
    while at < words.len() {
        let count = ((words[at] >> 16) as usize).max(1);
        found.push((words[at] & 0xffff, &words[at + 1..at + count]));
        at += count;
    }
    found
}

fn has_instruction(words: &[u32], opcode: u32) -> bool {
    instructions(words)
        .iter()
        .any(|(found, _)| *found == opcode)
}

#[test]
fn kept_overrides_are_spec_constants_with_ids() {
    let options = PassOptions {
        keep_overrides: true,
        ..PassOptions::default()
    };
    let units = units_with(
        include_str!("../../klf-shader-valid/tests/fixtures/options.ksir"),
        &options,
        false,
    );
    let words = &units[0].1;
    // `OpSpecConstant`, `OpDecorate _ SpecId 0` and `SpecId 1`, and the
    // `OpSpecConstantComposite` of the `WorkgroupSize` built-in.
    assert!(has_instruction(words, 50));
    let mut ids: Vec<u32> = instructions(words)
        .iter()
        .filter(|(opcode, operands)| *opcode == 71 && operands[1] == 1)
        .map(|(_, operands)| operands[2])
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, [0, 1]);
    assert!(has_instruction(words, 51));
    check_with_spirv_val("options", &units);
}

#[test]
fn debug_info_places_statements_in_their_source() {
    let source = include_str!("../../klf-shader-valid/tests/fixtures/bounds.ksir");
    let plain = units_with(source, &PassOptions::default(), false);
    let debug = units_with(source, &PassOptions::default(), true);
    assert!(!has_instruction(&plain[0].1, 8));
    for opcode in [7, 3, 8] {
        assert!(has_instruction(&debug[0].1, opcode), "opcode {opcode}");
    }
    check_with_spirv_val("bounds-debug", &debug);
}
