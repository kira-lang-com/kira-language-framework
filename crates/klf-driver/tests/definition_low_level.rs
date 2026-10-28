//! A definition builds a KIR module through the general low-level builder
//! services: an arbitrary composed type, a stack slot, element addresses,
//! stores, loads and an integer binary operator. Nothing in the definition
//! writes KIR text; the kernel holds the module.

use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

#[test]
fn definition_builds_a_module_through_the_low_level_services() {
    let source = r"
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
        let word = kir_type_integer(32)
        let pair = kir_type_array(2, word)
        let main_builder = kir_function_define(builder, 0, 4, word)
        let main_block = kir_function_block_new(main_builder)
        kir_function_branch(main_builder, main_block)
        kir_function_switch_block(main_builder, main_block)
        let slot = kir_value_stack_alloc(main_builder, pair)
        let zero = kir_value_integer(main_builder, word, 0)
        let one = kir_value_integer(main_builder, word, 1)
        let forty = kir_value_integer(main_builder, word, 40)
        let two = kir_value_integer(main_builder, word, 2)
        let first = kir_value_element_address(main_builder, slot, zero, word)
        kir_value_store(main_builder, forty, first)
        let second = kir_value_element_address(main_builder, slot, one, word)
        kir_value_store(main_builder, two, second)
        let left = kir_value_load(main_builder, first)
        let right = kir_value_load(main_builder, second)
        let sum = kir_value_binary(main_builder, 0, 0, 1, left, right)
        kir_function_result(main_builder, sum)
        return kir_module_finish(builder)
    }
}
";
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let mut image_session = Session::new(target.clone());
    let module = KiraDefinition
        .compile(
            &mut image_session,
            "definition.klfd",
            source,
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
    let result = definition
        .compile(&mut session, "input.calc", "main", "Main", execution_target)
        .unwrap_or_else(|| panic!("definition executes: {:?}", session.diagnostics()));
    assert_eq!(
        run(&result, "main", &[], Backend::Vm).expect("built module runs"),
        klf_vm::Outcome::Return(Some(42))
    );
}

#[test]
fn definition_builds_a_record_and_a_global() {
    // The unit source supplies the names: "main Point x y g".
    let source = r"
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
        let word = kir_type_integer(32)
        let point = kir_type_record(builder, 5, 10, 1)
        kir_type_record_field(point, 11, 12, word)
        kir_type_record_field(point, 13, 14, word)
        let counter = kir_global_new(builder, 15, 16, word, 0)
        kir_global_integer(counter, 40)
        let signature = kir_signature_new(builder, 0, 4, word)
        let main_builder = kir_signature_define(signature)
        let main_block = kir_function_block_new(main_builder)
        kir_function_branch(main_builder, main_block)
        kir_function_switch_block(main_builder, main_block)
        let slot = kir_value_stack_alloc(main_builder, point)
        let two = kir_value_integer(main_builder, word, 2)
        let first = kir_value_field_address(main_builder, slot, 0, word)
        kir_value_store(main_builder, two, first)
        let stored = kir_value_load(main_builder, first)
        let counter_address = kir_value_global_address(main_builder, counter)
        let base = kir_value_load(main_builder, counter_address)
        let sum = kir_value_binary(main_builder, 0, 0, 1, base, stored)
        kir_function_result(main_builder, sum)
        return kir_module_finish(builder)
    }
}
";
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let mut image_session = Session::new(target.clone());
    let module = KiraDefinition
        .compile(
            &mut image_session,
            "definition.klfd",
            source,
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
    let result = definition
        .compile(
            &mut session,
            "input.calc",
            "main Point x y g",
            "Main",
            execution_target,
        )
        .unwrap_or_else(|| panic!("definition executes: {:?}", session.diagnostics()));
    assert_eq!(
        run(&result, "main", &[], Backend::Vm).expect("built module runs"),
        klf_vm::Outcome::Return(Some(42))
    );
}
