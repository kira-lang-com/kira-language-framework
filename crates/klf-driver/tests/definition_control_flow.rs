//! A definition builds control flow structurally: blocks with parameters, a
//! loop driven by a conditional branch with independent edge arguments, a
//! `switch.int` with a default, a trap, an unreachable block, and a call to a
//! function that was declared before it had a body. No KIR text is involved.

use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

/// Byte ranges in the unit source below: `main` 0..4, `sum` 5..8, `step` 9..13
/// and the trap message `bad value` 14..23.
const UNIT: &str = "main sum step bad value";

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
        return definition_tree(session, unit, 42)
    }
    function elaborate(session: Session, unit: Unit, tree: Tree) -> Program {
        return definition_program(session, unit, definition_tree_payload(session, tree))
    }
    function lower(session: Session, unit: Unit, program: Program) -> KirModule {
        let builder = kir_module_new(session, unit, program)
        let word = kir_type_integer(64)

        let step_signature = kir_signature_new(builder, 9, 13, word)
        kir_signature_parameter(step_signature, word)
        let step = kir_signature_declare(step_signature)

        let sum_signature = kir_signature_new(builder, 5, 8, word)
        kir_signature_parameter(sum_signature, word)
        let sum = kir_signature_define(sum_signature)
        kir_block_parameter(sum, word)
        kir_block_parameter(sum, word)
        let loop_block = kir_block_define(sum)
        kir_block_parameter(sum, word)
        kir_block_parameter(sum, word)
        let body_block = kir_block_define(sum)
        kir_block_parameter(sum, word)
        let done_block = kir_block_define(sum)

        let limit = kir_function_parameter(sum, 0)
        let zero = kir_value_integer(sum, word, 0)
        kir_value_argument(sum, zero)
        kir_value_argument(sum, zero)
        kir_block_branch(sum, loop_block)

        kir_block_switch(sum, loop_block)
        let index = kir_block_value(sum, loop_block, 0)
        let total = kir_block_value(sum, loop_block, 1)
        let more = kir_value_compare(sum, 2, index, limit)
        kir_value_argument(sum, index)
        kir_value_argument(sum, total)
        kir_block_else_argument(sum, total)
        kir_block_conditional_branch(sum, more, body_block, done_block)

        kir_block_switch(sum, body_block)
        let body_index = kir_block_value(sum, body_block, 0)
        let body_total = kir_block_value(sum, body_block, 1)
        let next_total = kir_value_binary(sum, 0, 0, 1, body_total, body_index)
        kir_value_argument(sum, body_index)
        let next_index = kir_value_call(sum, step, word)
        kir_value_argument(sum, next_index)
        kir_value_argument(sum, next_total)
        kir_block_branch(sum, loop_block)

        kir_block_switch(sum, done_block)
        let result = kir_block_value(sum, done_block, 0)
        kir_function_result(sum, result)

        kir_function_body(step)
        let step_input = kir_function_parameter(step, 0)
        let one = kir_value_integer(step, word, 1)
        let stepped = kir_value_binary(step, 0, 0, 1, step_input, one)
        kir_function_result(step, stepped)

        let main_signature = kir_signature_new(builder, 0, 4, word)
        let main = kir_signature_define(main_signature)
        kir_block_parameter(main, word)
        let adjust_block = kir_block_define(main)
        let bad_block = kir_block_define(main)
        let dead_block = kir_block_define(main)

        let nine = kir_value_integer(main, word, 9)
        kir_value_argument(main, nine)
        let summed = kir_value_call(main, sum, word)
        let six = kir_value_integer(main, word, 6)
        kir_value_argument(main, six)
        kir_switch_case(main, 36, adjust_block)
        kir_switch_case(main, 0, dead_block)
        kir_switch_int(main, summed, bad_block)

        kir_block_switch(main, adjust_block)
        let adjustment = kir_block_value(main, adjust_block, 0)
        let answer = kir_value_binary(main, 0, 0, 1, summed, adjustment)
        kir_function_result(main, answer)

        kir_block_switch(main, bad_block)
        kir_block_trap(main, 14, 23)

        kir_block_switch(main, dead_block)
        kir_block_unreachable(main)

        return kir_module_finish(builder)
    }
}
";

fn build() -> klf_kir::Module {
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
    assert_eq!(image_session.diagnostics(), []);
    let image = DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), module)
        .expect("definition image validates");
    let mut session = Session::new(target);
    let definition =
        ExecutableDefinition::with_executor(image, BackendDefinitionExecutor::new(Backend::Vm));
    let execution_target = session.target().clone();
    definition
        .compile(&mut session, "input.calc", UNIT, "Main", execution_target)
        .unwrap_or_else(|| panic!("definition executes: {:?}", session.diagnostics()))
}

#[test]
fn definition_builds_blocks_branches_and_a_switch() {
    let module = build();
    let diagnostics = klf_kir_verify::verify_module(&module);
    assert!(diagnostics.is_empty(), "verifier: {diagnostics:?}");
    assert_eq!(
        run(&module, "main", &[], Backend::Vm).expect("built module runs"),
        klf_vm::Outcome::Return(Some(42))
    );
}
