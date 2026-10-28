use klf_driver::{Backend, BackendDefinitionExecutor, run_with_output, run_with_source};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

fn build_module(lower_body: &str, names: &str) -> klf_kir::Module {
    let source = format!(
        r"
construct Calc() extends Language<Tree, Program> {{
    function units(session: Session, inputs: [SourceFile]) -> [Unit] {{
        var result: [Unit] = []
        for file in inputs {{
            result.append(definition_unit(session, file))
        }}
        return result
    }}
    function parse(session: Session, unit: Unit) -> Tree {{
        return definition_tree(session, unit, 42)
    }}
    function elaborate(session: Session, unit: Unit, tree: Tree) -> Program {{
        return definition_program(session, unit, definition_tree_payload(session, tree))
    }}
    function lower(session: Session, unit: Unit, program: Program) -> KirModule {{
{lower_body}
    }}
}}
"
    );
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
        .unwrap_or_else(|| panic!("Kira definition lowers: {:?}", image_session.diagnostics()));
    assert_eq!(image_session.diagnostics(), []);
    let image = DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), module)
        .expect("Kira definition image validates");
    let mut session = Session::new(target);
    let definition =
        ExecutableDefinition::with_executor(image, BackendDefinitionExecutor::new(Backend::Vm));
    let execution_target = session.target().clone();
    definition
        .compile(&mut session, "input.calc", names, "Main", execution_target)
        .unwrap_or_else(|| panic!("Kira definition executes: {:?}", session.diagnostics()))
}

#[test]
fn compiled_kira_phase_body_emits_a_zero_arg_service_call() {
    let lower_body = r"        let builder = kir_module_new(session, unit, program)
        let main_builder = kir_function_new_i64(builder, 0, 4, 0)
        let main_block = kir_function_block_new(main_builder)
        kir_function_branch(main_builder, main_block)
        kir_function_switch_block(main_builder, main_block)
        let length = kir_value_call_service_i64(main_builder, 5, 22)
        kir_function_return(main_builder, length)
        return kir_module_finish(builder)";
    let module = build_module(lower_body, "main klfsvc_source_len klfsvc_source_byte");
    assert_eq!(
        run_with_source(&module, "main", &[], "ABCDE", Backend::Vm).expect("module runs"),
        klf_vm::Outcome::Return(Some(5))
    );
}

#[test]
fn compiled_kira_phase_body_emits_a_void_service_call() {
    let lower_body = r"        let builder = kir_module_new(session, unit, program)
        let main_builder = kir_function_new_i64(builder, 0, 4, 1)
        let main_block = kir_function_block_new(main_builder)
        kir_function_branch(main_builder, main_block)
        kir_function_switch_block(main_builder, main_block)
        let argument = kir_function_param_i64(main_builder, 0)
        kir_function_call_service_void_i64_1(main_builder, 42, 56, argument)
        let zero = kir_value_const_int(main_builder, 0)
        kir_function_return(main_builder, zero)
        return kir_module_finish(builder)";
    let module = build_module(
        lower_body,
        "main klfsvc_source_len klfsvc_source_byte klfsvc_io_puti",
    );
    let (outcome, output) =
        run_with_output(&module, "main", &[5], "", Backend::Vm).expect("module runs");
    assert_eq!(outcome, klf_vm::Outcome::Return(Some(0)));
    assert_eq!(output, b"5\n");
}
