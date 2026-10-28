use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

fn run_terminator_definition(lower_body: &str, names: &str) -> klf_vm::Outcome {
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
    let result = definition
        .compile(&mut session, "input.calc", names, "Main", execution_target)
        .unwrap_or_else(|| panic!("Kira definition executes: {:?}", session.diagnostics()));
    run(&result, "main", &[5], Backend::Vm).expect("Kira-built module runs")
}

#[test]
fn compiled_kira_phase_body_emits_a_dead_trap() {
    let lower_body = r"        let builder = kir_module_new(session, unit, program)
        let main_builder = kir_function_new_i64(builder, 0, 4, 1)
        let main_block = kir_function_block_new(main_builder)
        kir_function_branch(main_builder, main_block)
        kir_function_switch_block(main_builder, main_block)
        let argument = kir_function_param_i64(main_builder, 0)
        let condition = kir_value_const_bool(main_builder, 1)
        let then_block = kir_function_block_new_i64(main_builder)
        let else_block = kir_function_block_new_i64(main_builder)
        let then_value = kir_function_block_param_i64(main_builder, then_block, 0)
        let else_value = kir_function_block_param_i64(main_builder, else_block, 0)
        let fallback = kir_value_const_int(main_builder, 0)
        kir_function_conditional_branch_i64(main_builder, condition, then_block, argument, else_block, fallback)
        kir_function_switch_block(main_builder, then_block)
        kir_function_return(main_builder, then_value)
        kir_function_switch_block(main_builder, else_block)
        kir_function_trap(main_builder, 5, 10)
        return kir_module_finish(builder)";
    assert_eq!(
        run_terminator_definition(lower_body, "main abort"),
        klf_vm::Outcome::Return(Some(5))
    );
}
