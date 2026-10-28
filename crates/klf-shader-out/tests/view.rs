//! The reader and writer services over the section 18 fixtures.
#![allow(clippy::cast_possible_wrap)]

use klf_shader::Target;
use klf_shader_out::{UnitView, Written};
use klf_shader_passes::PassOptions;
use klf_shader_valid::{Options, validate};

fn views(source: &str, target: Target) -> Vec<UnitView> {
    let mut module = klf_shader::parse::parse(source).expect("the fixture parses");
    klf_shader_passes::normalize(&mut module);
    let options = Options { wgsl: false };
    assert!(validate(&module, options).is_valid());
    let assignment = klf_shader_bind::assign(&module, 0).expect("the slots are assigned");
    let (units, _) =
        klf_shader_passes::run(&module, target, &PassOptions::default()).expect("the passes run");
    units
        .into_iter()
        .map(|unit| {
            let validation = validate(&unit.module, Options { wgsl: false });
            let info = validation.info.expect("the unit validates");
            let resources = assignment.for_unit(&module, &unit.module);
            UnitView::new(unit.module, &info, &resources, target, false)
        })
        .collect()
}

fn text(view: &mut UnitView, id: i64) -> String {
    view.text_of(id).expect("a text id").to_owned()
}

fn scan() -> UnitView {
    let mut views = views(
        include_str!("../../klf-shader-valid/tests/fixtures/scan.ksir"),
        Target::Ksl,
    );
    assert_eq!(views.len(), 1);
    views.remove(0)
}

#[test]
fn the_unit_and_its_globals_read_back() {
    let mut view = scan();
    let kind = view.kind("unit", -1, 0);
    assert_eq!(text(&mut view, kind), "compute");
    let shader = view.text("unit", -1, 0, "shader", 0);
    assert_eq!(text(&mut view, shader), "Scan");
    assert_eq!(view.int("unit", -1, 0, "entries", -1), 1);
    assert_eq!(view.count("globals", -1), 3);
    let space = view.text("globals", -1, 1, "space", 0);
    assert_eq!(text(&mut view, space), "storage");
    let access = view.text("globals", -1, 1, "access", 0);
    assert_eq!(text(&mut view, access), "load store atomic");
    assert_eq!(view.int("globals", -1, 1, "slot", 0), 1);
    assert_eq!(view.int("globals", -1, 2, "slot", 0), -1);
    assert_eq!(view.int("globals", -1, 2, "used", 0), 1);
    let class = view.kind("groups", -1, 0);
    assert_eq!(text(&mut view, class), "dispatch");
    assert!(matches!(view.finish(true), Written::Output(text) if text.is_empty()));
}

#[test]
fn functions_blocks_and_expressions_read_back() {
    let mut view = scan();
    let entry = view.int("unit", -1, 0, "entries", 0);
    let stage = view.text("functions", -1, entry, "stage", 0);
    assert_eq!(text(&mut view, stage), "compute");
    assert_eq!(view.int("functions", -1, entry, "workgroup", 0), 64);
    let body = view.int("functions", -1, entry, "body", 0);
    let statements = view.count("statements", body);
    let kinds: Vec<String> = (0..statements)
        .map(|index| {
            let kind = view.kind("statements", body, index);
            text(&mut view, kind)
        })
        .collect();
    // Workgroup zeroing comes first, so the shader's own `if` is the one
    // right before its barrier.
    let barrier = kinds
        .iter()
        .rposition(|kind| kind == "control_barrier")
        .expect("a barrier");
    assert_eq!(kinds[barrier - 1], "if");
    let guard = barrier as i64 - 1;
    let accept = view.int("statements", body, guard, "accept", 0);
    assert_eq!(view.count("statements", accept), 3);
    let condition = view.int("statements", body, guard, "condition", 0);
    let op = view.text("expressions", entry, condition, "op", 0);
    assert_eq!(text(&mut view, op), "less");
    let ty = view.int("expressions", entry, condition, "type", 0);
    let kind = view.kind("types", -1, ty);
    assert_eq!(text(&mut view, kind), "scalar");
    let scalar = view.text("types", -1, ty, "scalar", 0);
    assert_eq!(text(&mut view, scalar), "bool");
}

#[test]
fn loads_bake_and_single_use_arithmetic_does_not() {
    let mut view = scan();
    let entry = view.int("unit", -1, 0, "entries", 0);
    let expressions = view.count("expressions", entry);
    let mut baked = Vec::new();
    for index in 0..expressions {
        if view.int("expressions", entry, index, "bake", 0) == 1 {
            let kind = view.kind("expressions", entry, index);
            baked.push(text(&mut view, kind));
        }
    }
    assert!(baked.iter().any(|kind| kind == "load"));
    assert!(baked.iter().all(|kind| kind != "access"));
}

#[test]
fn writes_join_and_refusals_win() {
    let mut view = scan();
    view.write("a");
    view.write("b");
    assert!(matches!(view.finish(true), Written::Output(text) if text == "ab"));
    let mut view = scan();
    view.write("a");
    view.refuse("atomics", -1, -1);
    match view.finish(false) {
        Written::Refused(errors) => {
            assert_eq!(errors[0].key, Some("shader.target-unsupported"));
            assert_eq!(errors[0].message, "ksl: atomics");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn the_first_misuse_is_kept() {
    let mut view = scan();
    assert_eq!(view.count("nothing", -1), -1);
    assert_eq!(view.int("globals", -1, 0, "colour", 0), -1);
    match view.finish(true) {
        Written::Misused(message) => assert!(message.contains("no table `nothing`")),
        other => panic!("expected a misuse, got {other:?}"),
    }
    let mut view = scan();
    assert_eq!(view.int("globals", -1, 0, "init", 0), -1);
    assert_eq!(view.int("globals", -1, 9, "type", 0), -1);
    assert!(
        matches!(view.finish(true), Written::Misused(message) if message.contains("no record 9"))
    );
}
