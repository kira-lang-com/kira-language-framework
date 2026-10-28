//! Definition-facing string constants use the same service path as all other
//! low-level KIR construction.

use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

#[test]
fn definition_builds_a_string_constant_through_the_low_level_service() {
    let template = r"
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
        let signature = kir_signature_new(builder, 0, 4, word)
        let main = kir_signature_define(signature)
        let entry = kir_function_block_new(main)
        kir_function_branch(main, entry)
        kir_function_switch_block(main, entry)
        let pointer = kir_value_const_string(main, {START}, {END})
        let length = kir_value_const_string_length(main, pointer)
        kir_function_result(main, length)
        return kir_module_finish(builder)
    }
}
";
    let source = template.replace("{START}", "4").replace("{END}", "9");
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let mut image_session = Session::new(target.clone());
    let module = KiraDefinition
        .compile(
            &mut image_session,
            "definition.klfd",
            &source,
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
            "main\"KLF\"",
            "Main",
            execution_target,
        )
        .unwrap_or_else(|| panic!("definition executes: {:?}", session.diagnostics()));
    assert_eq!(
        run(&result, "main", &[], Backend::Vm).expect("built module runs"),
        klf_vm::Outcome::Return(Some(3))
    );
}
