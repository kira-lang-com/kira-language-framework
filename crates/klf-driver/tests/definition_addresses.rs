//! A definition hoists its stack slots into the entry block, crosses between
//! the address and value handle spaces, and reads an anonymous global. These
//! are the pieces a frontend needs when its own values are indistinguishably
//! addresses and values, and when a literal has no name in the source.

use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

/// Byte ranges in the unit source below: `main` 0..4.
const UNIT: &str = "main";

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
        let byte = kir_type_integer(8)
        let bytes = kir_type_array(4, byte)

        let table = kir_global_anonymous(builder, bytes, 0)
        kir_global_byte(table, 10)
        kir_global_byte(table, 20)
        kir_global_byte(table, 30)
        kir_global_byte(table, 40)

        let signature = kir_signature_new(builder, 0, 4, word)
        let main = kir_signature_define(signature)
        let entry = kir_block_entry(main)
        let body = kir_block_define(main)
        kir_block_switch(main, body)

        kir_block_switch(main, entry)
        let slot = kir_value_stack_alloc(main, word)
        kir_block_switch(main, body)

        let two = kir_value_integer(main, word, 2)
        kir_value_store(main, two, slot)
        let held = kir_value_from_address(main, slot)
        let reread = kir_address_from_value(main, held, word)
        let index = kir_value_load(main, reread)

        let base = kir_value_global_address(main, table)
        let cell = kir_value_element_address(main, base, index, byte)
        let entry_value = kir_value_load(main, cell)
        let wide = kir_value_convert(main, 1, entry_value, word)
        kir_value_stack_dealloc(main, slot)
        kir_function_result(main, wide)

        kir_block_switch(main, entry)
        kir_block_branch(main, body)
        return kir_module_finish(builder)
    }
}
";

#[test]
fn a_definition_hoists_slots_and_crosses_the_handle_spaces() {
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
    assert_eq!(
        run(&built, "main", &[], Backend::Vm).expect("built module runs"),
        klf_vm::Outcome::Return(Some(30))
    );
}
