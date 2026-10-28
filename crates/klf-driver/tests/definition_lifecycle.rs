//! Memberwise lifecycle synthesis (section 11.2): a definition-built record
//! with a counted member gets a synthesized `destroy` and an implicit `copy`,
//! and a record holding that record calls them.

use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

#[test]
fn managed_members_get_memberwise_copy_and_destroy() {
    // The unit source supplies the names: "main Node r n Holder h".
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
        return definition_tree(session, unit, 0)
    }
    function elaborate(session: Session, unit: Unit, tree: Tree) -> Program {
        return definition_program(session, unit, 0)
    }
    function lower(session: Session, unit: Unit, program: Program) -> KirModule {
        let builder = kir_module_new(session, unit, program)
        let word = kir_type_integer(64)
        let counted = kir_type_reference(word)
        let node = kir_type_record(builder, 5, 9, 1)
        kir_type_record_field(node, 10, 11, counted)
        kir_type_record_field(node, 12, 13, word)
        let holder = kir_type_record(builder, 14, 20, 1)
        kir_type_record_field(holder, 21, 22, node)
        let main_builder = kir_function_define(builder, 0, 4, word)
        let main_block = kir_function_block_new(main_builder)
        kir_function_branch(main_builder, main_block)
        kir_function_switch_block(main_builder, main_block)
        kir_function_result(main_builder, kir_value_integer(main_builder, word, 7))
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
            "main Node r n Holder h",
            "Main",
            execution_target,
        )
        .unwrap_or_else(|| panic!("definition executes: {:?}", session.diagnostics()));
    let text = result.to_string();
    assert!(
        text.contains("copy: implicit(@__klf_copy_Node), destroy: fn(@__klf_destroy_Node)"),
        "{text}"
    );
    assert!(
        text.contains("copy: implicit(@__klf_copy_Holder), destroy: fn(@__klf_destroy_Holder)"),
        "{text}"
    );
    assert!(text.contains("call @__klf_destroy_Node("), "{text}");
    assert!(text.contains("load [copy]"), "{text}");
    let diagnostics = klf_kir_verify::verify_module(&result);
    assert!(diagnostics.is_empty(), "{diagnostics:?}\n{text}");
    assert_eq!(
        run(&result, "main", &[], Backend::Vm).expect("built module runs"),
        klf_vm::Outcome::Return(Some(7))
    );
}
