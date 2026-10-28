//! The bounds and override passes (sections 15.5 and 17.3) and capability
//! refusal (section 16.6) on fixtures the `ksl` definition lowered.

use klf_shader::{
    Capability, Expression, Literal, MathFunction, Module, Scalar, Statement, Target,
};
use klf_shader_passes::{Bounds, PassOptions, Unit};
use klf_shader_valid::{Options, ValidationError, validate};

const BOUNDS: &str = include_str!("../../klf-shader-valid/tests/fixtures/bounds.ksir");
const OPTIONS: &str = include_str!("../../klf-shader-valid/tests/fixtures/options.ksir");

fn run(
    source: &str,
    target: Target,
    options: &PassOptions,
) -> Result<Vec<Unit>, Vec<ValidationError>> {
    let mut module = klf_shader::parse::parse(source).expect("the fixture parses");
    klf_shader_passes::normalize(&mut module);
    let validation = validate(&module, Options { wgsl: true });
    assert!(validation.is_valid(), "{:?}", validation.problems);
    let (units, _) = klf_shader_passes::run(&module, target, options)?;
    for unit in &units {
        let validation = validate(&unit.module, Options { wgsl: true });
        assert!(validation.is_valid(), "{:?}", validation.problems);
    }
    Ok(units)
}

fn bounded(bounds: Bounds) -> Module {
    let options = PassOptions {
        bounds,
        ..PassOptions::default()
    };
    let units = run(BOUNDS, Target::Wgsl, &options).expect("the passes run");
    units.into_iter().next().expect("one unit").module
}

fn count(module: &Module, wanted: impl Fn(&Expression) -> bool) -> usize {
    let entry = &module.entry_points[0].function;
    entry
        .expressions
        .iter()
        .filter(|(_, expression)| wanted(expression))
        .count()
}

/// The statements of the entry's body that are guarded stores: an `if`
/// holding only a store.
fn guarded_stores(module: &Module) -> usize {
    module.entry_points[0]
        .function
        .body
        .iter()
        .filter(|(statement, _)| match statement {
            Statement::If { accept, reject, .. } => {
                reject.is_empty()
                    && accept.iter().all(|(inner, _)| {
                        matches!(
                            inner,
                            Statement::Store { .. } | Statement::ImageStore { .. }
                        )
                    })
            }
            _ => false,
        })
        .count()
}

fn is_min(expression: &Expression) -> bool {
    matches!(
        expression,
        Expression::Math {
            fun: MathFunction::Min,
            ..
        }
    )
}

#[test]
fn unchecked_leaves_indexing_alone() {
    let module = bounded(Bounds::Unchecked);
    assert_eq!(count(&module, is_min), 0);
    assert_eq!(guarded_stores(&module), 0);
}

#[test]
fn restrict_clamps_every_dynamic_index_and_texel() {
    let module = bounded(Bounds::Restrict);
    // Four buffer indices, one vector lane, one load and one store texel.
    assert_eq!(count(&module, is_min), 7);
    assert_eq!(
        count(&module, |e| matches!(e, Expression::Select { .. })),
        0
    );
    assert_eq!(guarded_stores(&module), 0);
}

#[test]
fn read_zero_skip_write_zeroes_reads_and_drops_writes() {
    let module = bounded(Bounds::ReadZeroSkipWrite);
    assert_eq!(count(&module, is_min), 7);
    // Three scalar reads and the texel load; the whole `Item` read is only
    // clamped.
    assert_eq!(
        count(&module, |e| matches!(e, Expression::Select { .. })),
        4
    );
    assert_eq!(guarded_stores(&module), 2);
}

#[test]
fn kept_overrides_reach_targets_that_specialize() {
    let options = PassOptions {
        keep_overrides: true,
        ..PassOptions::default()
    };
    for target in [Target::Msl, Target::Spirv, Target::Wgsl] {
        let units = run(OPTIONS, target, &options).expect("the passes run");
        for unit in units {
            assert_eq!(unit.module.overrides.len(), 2, "{target:?}");
        }
    }
}

#[test]
fn kept_overrides_are_refused_where_they_cannot_be_specialized() {
    let options = PassOptions {
        keep_overrides: true,
        ..PassOptions::default()
    };
    for target in [Target::Hlsl, Target::Glsl] {
        let problems = run(OPTIONS, target, &options).expect_err("the target cannot keep them");
        assert_eq!(problems.len(), 2);
        assert!(
            problems
                .iter()
                .all(|problem| problem.key == Some("shader.override-unsupported"))
        );
    }
}

#[test]
fn baked_overrides_are_gone() {
    let units = run(OPTIONS, Target::Hlsl, &PassOptions::default()).expect("the passes run");
    assert!(units[0].module.overrides.is_empty());
}

#[test]
fn a_declared_capability_the_module_does_not_use_is_dropped() {
    let mut module = klf_shader::parse::parse(OPTIONS).expect("the fixture parses");
    module.capabilities.insert(Capability::Float16);
    let used = klf_shader_valid::used_capabilities(&module);
    assert!(!used.contains(&Capability::Float16));
}

#[test]
fn a_capability_the_target_cannot_express_is_refused() {
    let mut module = klf_shader::parse::parse(OPTIONS).expect("the fixture parses");
    klf_shader_passes::normalize(&mut module);
    module.capabilities.insert(Capability::Int64);
    for target in [Target::Glsl, Target::Wgsl, Target::Msl] {
        let result = klf_shader_passes::run(&module, target, &PassOptions::default());
        if target == Target::Msl {
            assert!(result.is_ok(), "{target:?} expresses int64");
        } else {
            let problems = result.expect_err("no int64");
            assert_eq!(problems.len(), 1);
            assert_eq!(problems[0].key, Some("shader.capability"));
        }
    }
}

const HALF: &str = include_str!("../../klf-shader-valid/tests/fixtures/half.ksir");

#[test]
fn halves_stay_where_the_target_has_them() {
    let units = run(HALF, Target::Wgsl, &PassOptions::default()).expect("the passes run");
    let module = &units[0].module;
    assert!(
        module
            .types
            .iter()
            .any(|(_, ty)| ty.inner.scalar() == Some(Scalar::F16))
    );
    assert_eq!(
        count(module, |e| matches!(
            e,
            Expression::Math {
                fun: MathFunction::Pack2x16Float,
                ..
            }
        )),
        0
    );
}

#[test]
fn halves_become_floats_rounded_to_halves_on_glsl() {
    let units = run(HALF, Target::Glsl, &PassOptions::default()).expect("the passes run");
    let module = &units[0].module;
    assert!(!module.capabilities.contains(&Capability::Float16));
    assert!(
        module
            .types
            .iter()
            .all(|(_, ty)| ty.inner.scalar() != Some(Scalar::F16))
    );
    // The convert and the two arithmetic results, four lanes each.
    let packs = count(module, |e| {
        matches!(
            e,
            Expression::Math {
                fun: MathFunction::Pack2x16Float,
                ..
            }
        )
    });
    assert_eq!(packs, 12);
    let halves = count(module, |e| {
        matches!(e, Expression::Literal(Literal::F16(_)))
    });
    assert_eq!(halves, 0);
}
