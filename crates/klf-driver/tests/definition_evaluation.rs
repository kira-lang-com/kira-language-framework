use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

/// A definition whose `parse` builds functions and evaluates them at compile
/// time.
const SOURCE: &str = r#"
construct Calc() extends Language<Tree, Program> {
    function units(session: Session, inputs: [SourceFile]) -> [Unit] {
        return [definition_unit(session, inputs[0])]
    }
    function parse(session: Session, unit: Unit) -> Tree {
        let scalar = kir_module_detached(session, unit)
        let word = kir_type_integer(64)
        let signature = kir_signature_new(scalar, 0, 4, word)
        let main = kir_signature_define(signature)
        let block = kir_function_block_new(main)
        kir_function_branch(main, block)
        kir_function_switch_block(main, block)
        kir_function_result(main, kir_value_integer(main, word, 42))
        let result = kir_module_evaluate(scalar, main, 10000, 1048576)
        if kir_evaluation_status(result) != 1 { return definition_tree(session, unit, 1) }
        let value = kir_evaluation_value(result)

        let limited = kir_module_detached(session, unit)
        let limitedSignature = kir_signature_new(limited, 0, 4, word)
        let limitedMain = kir_signature_define(limitedSignature)
        let limitedBlock = kir_function_block_new(limitedMain)
        kir_function_branch(limitedMain, limitedBlock)
        kir_function_switch_block(limitedMain, limitedBlock)
        kir_function_result(limitedMain, kir_value_integer(limitedMain, word, 99))
        let exhausted = kir_module_evaluate(limited, limitedMain, 0, 1048576)
        if kir_evaluation_status(exhausted) != 0 { return definition_tree(session, unit, 2) }
        if kir_evaluation_length(exhausted) == 0 { return definition_tree(session, unit, 3) }

        let buffer = kir_module_detached(session, unit)
        let pointer = kir_type_pointer()
        let record = kir_type_record(buffer, 5, 9, 0)
        kir_type_record_field(record, 5, 9, pointer)
        kir_type_record_field(record, 10, 16, word)
        let bufferSignature = kir_signature_new(buffer, 0, 4, kir_type_unit())
        kir_signature_parameter(bufferSignature, kir_type_address(record))
        let bufferMain = kir_signature_define(bufferSignature)
        let bufferBlock = kir_function_block_new(bufferMain)
        kir_function_branch(bufferMain, bufferBlock)
        kir_function_switch_block(bufferMain, bufferBlock)
        let out = kir_address_from_value(bufferMain, kir_function_parameter(bufferMain, 0), record)
        kir_value_text_byte(bufferMain, 75)
        kir_value_text_byte(bufferMain, 76)
        kir_value_text_byte(bufferMain, 70)
        let bytes = kir_value_text(bufferMain)
        kir_value_store(bufferMain, bytes, kir_value_field_address(bufferMain, out, 0, pointer))
        kir_value_store(bufferMain, kir_value_const_string_length(bufferMain, bytes), kir_value_field_address(bufferMain, out, 1, word))
        kir_function_result_void(bufferMain)
        let captured = kir_module_evaluate_buffer(buffer, bufferMain, 0, 1, 10000, 1048576)
        if kir_evaluation_status(captured) != 1 { return definition_tree(session, unit, 4) }
        if kir_evaluation_length(captured) != 3 { return definition_tree(session, unit, 5) }
        if kir_evaluation_byte(captured, 0) != 75 { return definition_tree(session, unit, 6) }
        if kir_evaluation_byte(captured, 2) != 70 { return definition_tree(session, unit, 7) }
        let origin = 0 as SourceFile
        let generated = kir_evaluation_source(session, origin, captured)
        if fileText(session, generated as SourceFile) != "KLF" { return definition_tree(session, unit, 8) }
        let inline = addSource(session, origin, "42")
        if fileText(session, inline as SourceFile) != "42" { return definition_tree(session, unit, 9) }
        return definition_tree(session, unit, value)
    }
    function elaborate(session: Session, unit: Unit, tree: Tree) -> Program {
        return definition_program(session, unit, definition_tree_payload(session, tree))
    }
    function lower(session: Session, unit: Unit, program: Program) -> KirModule {
        let builder = kir_module_new(session, unit, program)
        let word = kir_type_integer(64)
        let signature = kir_signature_new(builder, 0, 4, word)
        let main = kir_signature_define(signature)
        let block = kir_function_block_new(main)
        kir_function_branch(main, block)
        kir_function_switch_block(main, block)
        kir_function_result(main, kir_value_integer(main, word, definition_program_payload(session, program)))
        return kir_module_finish(builder)
    }
}
"#;

#[test]
fn definition_evaluates_scalar_and_buffer_during_parse() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .unwrap();
    let mut image_session = Session::new(target.clone());
    let module = KiraDefinition
        .compile(
            &mut image_session,
            "evaluation.klfd",
            SOURCE,
            "Definition",
            target.clone(),
        )
        .unwrap_or_else(|| panic!("definition lowers: {:?}", image_session.diagnostics()));
    let image =
        DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), module).unwrap();
    let definition =
        ExecutableDefinition::with_executor(image, BackendDefinitionExecutor::new(Backend::Vm));
    let mut session = Session::new(target.clone());
    let module = definition
        .compile(
            &mut session,
            "input.calc",
            "main data length",
            "Main",
            target,
        )
        .unwrap_or_else(|| panic!("definition executes: {:?}", session.diagnostics()));
    assert_eq!(
        run(&module, "main", &[], Backend::Vm).unwrap(),
        klf_vm::Outcome::Return(Some(42))
    );
}
