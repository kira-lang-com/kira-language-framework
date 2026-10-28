//! A definition declares an enum by handle, builds a value with a payload in
//! memory, and dispatches on it with `switch.enum`: the low-level enum slice a
//! frontend with its own sum types lowers through.

use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

/// Byte ranges in the unit source below: `Shape` 0..5, `Circle` 6..12,
/// `Square` 13..19, `main` 20..24.
const UNIT: &str = "Shape Circle Square main";

const DEFINITION: &str = r"
construct Calc() extends Language<Tree, Program> {
    function units(session: Session, inputs: [SourceFile]) -> [Unit] {
        var result: [Unit] = []
        for file in inputs {
            result.append(definition_unit(session, file))
        }
        return result
    }
    function parse(session: Session, unit: Unit) -> Tree {
        return definition_tree(session, unit, 0)
    }
    function elaborate(session: Session, unit: Unit, tree: Tree) -> Program {
        return definition_program(session, unit, definition_tree_payload(session, tree))
    }
    function lower(session: Session, unit: Unit, program: Program) -> KirModule {
        let builder = kir_module_new(session, unit, program)
        let word = kir_type_integer(64)
        let shape = kir_type_enum_named(builder, 0, 5, 0)
        kir_type_enum_case(shape, 6, 12, kir_type_unit())
        kir_type_enum_case(shape, 13, 19, word)

        let signature = kir_signature_new(builder, 20, 24, word)
        let main = kir_signature_define(signature)
        let entry = kir_block_entry(main)
        let body = kir_block_define(main)
        kir_block_switch(main, entry)
        let slot = kir_value_stack_alloc(main, shape)
        kir_block_switch(main, body)

        let payload = kir_value_enum_payload_address(main, slot, 1, word)
        kir_value_store(main, kir_value_integer(main, word, 7), payload)
        kir_value_enum_inject(main, slot, 1)

        let circle = kir_block_define(main)
        let square = kir_block_define(main)
        kir_switch_case(main, 0, circle)
        kir_switch_case(main, 1, square)
        kir_switch_enum(main, slot)

        kir_block_switch(main, circle)
        kir_value_stack_dealloc(main, slot)
        kir_function_result(main, kir_value_integer(main, word, 0))

        kir_block_switch(main, square)
        let tag = kir_value_enum_tag(main, slot)
        let side = kir_value_load(main, kir_value_enum_payload_address(main, slot, 1, word))
        let scaled = kir_value_binary(main, 2, 1, 1, tag, kir_value_integer(main, word, 100))
        let total = kir_value_binary(main, 0, 1, 1, scaled, side)
        kir_value_stack_dealloc(main, slot)
        kir_function_result(main, total)

        kir_block_switch(main, entry)
        kir_block_branch(main, body)
        return kir_module_finish(builder)
    }
}
";

#[test]
fn a_definition_builds_and_switches_on_an_enum() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let mut image_session = Session::new(target.clone());
    let module = KiraDefinition
        .compile(
            &mut image_session,
            "definition.klfd",
            DEFINITION,
            "Definition",
            target.clone(),
        )
        .unwrap_or_else(|| panic!("definition lowers: {:?}", image_session.diagnostics()));
    let image = DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), module)
        .expect("definition image validates");
    let mut session = Session::new(target);
    let definition =
        ExecutableDefinition::with_executor(image, BackendDefinitionExecutor::new(Backend::Vm));
    let execution_target = session.target().clone();
    let built = definition
        .compile(&mut session, "input.calc", UNIT, "Main", execution_target)
        .unwrap_or_else(|| panic!("definition executes: {:?}", session.diagnostics()));

    let diagnostics = klf_kir_verify::verify_module(&built);
    assert!(diagnostics.is_empty(), "verifier: {diagnostics:?}");
    for backend in [Backend::Vm, Backend::Native] {
        assert_eq!(
            run(&built, "main", &[], backend).expect("built module runs"),
            klf_vm::Outcome::Return(Some(107)),
            "{backend:?}"
        );
    }
}
