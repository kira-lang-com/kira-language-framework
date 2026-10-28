//! The validator on the section 18.2 example and on broken variants of it.

use klf_shader::{Expression, Statement, parse::parse, print::print};
use klf_shader_valid::{Options, validate};

fn tri() -> klf_shader::Module {
    parse(include_str!("fixtures/tri.ksir")).expect("the example parses")
}

fn rules(module: &klf_shader::Module) -> Vec<&'static str> {
    validate(module, Options { wgsl: true })
        .problems
        .iter()
        .map(|problem| problem.rule)
        .collect()
}

#[test]
fn the_section_18_example_is_valid_and_round_trips() {
    let module = tri();
    let validation = validate(&module, Options { wgsl: true });
    assert!(validation.is_valid(), "{:?}", validation.problems);
    let info = validation.info.expect("info");
    let uses = info.entry_globals(&module, 0);
    assert!(
        uses[0].read && !uses[0].write,
        "the vertex entry reads the camera"
    );
    assert!(
        !info.entry_globals(&module, 1)[0].any(),
        "the fragment entry does not"
    );
    let text = print(&module);
    assert_eq!(print(&parse(&text).expect("printed text parses")), text);
}

#[test]
fn an_expression_emitted_twice_is_sv7() {
    let mut module = tri();
    let body = &mut module.entry_points[1].function.body;
    let first = body.statements[0].clone();
    body.statements.insert(1, first);
    body.spans.insert(1, klf_shader::Span::default());
    assert!(rules(&module).contains(&"SV7"));
}

#[test]
fn a_store_through_a_uniform_is_sv9() {
    let mut module = tri();
    let function = &mut module.entry_points[0].function;
    let Some(Statement::Store { pointer, .. }) = function.body.statements.get_mut(3) else {
        panic!("statement 2 is the store to `out.clip`");
    };
    *pointer = klf_shader::Handle::new(10);
    assert!(rules(&module).contains(&"SV9"), "{:?}", rules(&module));
}

#[test]
fn a_missing_return_is_sv11_and_a_bad_location_is_sv18() {
    let mut module = tri();
    let body = &mut module.entry_points[1].function.body;
    body.statements.pop();
    body.spans.pop();
    assert!(rules(&module).contains(&"SV11"));

    let mut module = tri();
    let fragment_input = module.entry_points[1].function.arguments[0].ty;
    let mut ty = module.types[fragment_input].clone();
    if let klf_shader::TypeInner::Struct { members, .. } = &mut ty.inner {
        members[1].binding = Some(klf_shader::Binding::Location {
            location: Some(3),
            interpolation: None,
            sampling: None,
            blend_src: None,
        });
    }
    ty.name = Some("FragmentIn".to_owned());
    module.entry_points[1].function.arguments[0].ty =
        module.types.insert(ty, klf_shader::Span::default());
    let problems = validate(&module, Options::default()).problems;
    assert!(
        problems
            .iter()
            .any(|problem| problem.key == Some("shader.interface-mismatch")),
        "{problems:?}"
    );
}

#[test]
fn a_constant_index_out_of_range_is_keyed() {
    let mut module = tri();
    let function = &mut module.entry_points[0].function;
    function.expressions[klf_shader::Handle::new(15)] = Expression::AccessIndex {
        base: klf_shader::Handle::new(9),
        index: 7,
    };
    let problems = validate(&module, Options::default()).problems;
    assert!(
        problems
            .iter()
            .any(|problem| problem.key == Some("shader.const-index")),
        "{problems:?}"
    );
}
