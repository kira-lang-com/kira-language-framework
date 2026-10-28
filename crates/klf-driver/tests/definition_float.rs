use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

#[test]
fn compiled_kira_phase_body_computes_with_f64_values() {
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
        let main_builder = kir_function_new_i64(builder, 0, 4, 1)
        let main_block = kir_function_block_new(main_builder)
        kir_function_branch(main_builder, main_block)
        kir_function_switch_block(main_builder, main_block)
        let argument = kir_function_param_i64(main_builder, 0)
        let base = kir_value_i64_to_f64(main_builder, argument)
        let three = kir_value_const_int(main_builder, 3)
        let three_f = kir_value_i64_to_f64(main_builder, three)
        let sum = kir_value_add_f64(main_builder, base, three_f)
        let difference = kir_value_sub_f64(main_builder, sum, three_f)
        let product = kir_value_mul_f64(main_builder, difference, three_f)
        let quotient = kir_value_div_f64(main_builder, product, three_f)
        let result = kir_value_f64_to_i64(main_builder, quotient)
        kir_function_return(main_builder, result)
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
        .unwrap_or_else(|| panic!("Kira definition lowers: {:?}", image_session.diagnostics()));
    assert_eq!(image_session.diagnostics(), []);
    let image = DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), module)
        .expect("Kira definition image validates");
    let mut session = Session::new(target);
    let definition =
        ExecutableDefinition::with_executor(image, BackendDefinitionExecutor::new(Backend::Vm));
    let execution_target = session.target().clone();
    let result = definition
        .compile(&mut session, "input.calc", "main", "Main", execution_target)
        .unwrap_or_else(|| panic!("Kira definition executes: {:?}", session.diagnostics()));
    assert_eq!(
        run(&result, "main", &[12], Backend::Vm).expect("Kira-built module runs"),
        klf_vm::Outcome::Return(Some(12))
    );
}

#[test]
fn compiled_kira_phase_body_branches_on_an_f64_comparison() {
    // 4612811918334230528 == 2.5f64.to_bits()
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
        let main_builder = kir_function_new_i64(builder, 0, 4, 1)
        let main_block = kir_function_block_new(main_builder)
        kir_function_branch(main_builder, main_block)
        kir_function_switch_block(main_builder, main_block)
        let argument = kir_function_param_i64(main_builder, 0)
        let value = kir_value_i64_to_f64(main_builder, argument)
        let threshold = kir_value_const_f64(main_builder, 4612811918334230528)
        let below = kir_value_lt_f64(main_builder, value, threshold)
        let then_block = kir_function_block_new_i64(main_builder)
        let else_block = kir_function_block_new_i64(main_builder)
        let then_value = kir_function_block_param_i64(main_builder, then_block, 0)
        let else_value = kir_function_block_param_i64(main_builder, else_block, 0)
        let one = kir_value_const_int(main_builder, 1)
        let zero = kir_value_const_int(main_builder, 0)
        kir_function_conditional_branch_i64(main_builder, below, then_block, one, else_block, zero)
        kir_function_switch_block(main_builder, then_block)
        kir_function_return(main_builder, then_value)
        kir_function_switch_block(main_builder, else_block)
        kir_function_return(main_builder, else_value)
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
        .unwrap_or_else(|| panic!("Kira definition lowers: {:?}", image_session.diagnostics()));
    assert_eq!(image_session.diagnostics(), []);
    let image = DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), module)
        .expect("Kira definition image validates");
    let mut session = Session::new(target);
    let definition =
        ExecutableDefinition::with_executor(image, BackendDefinitionExecutor::new(Backend::Vm));
    let execution_target = session.target().clone();
    let result = definition
        .compile(&mut session, "input.calc", "main", "Main", execution_target)
        .unwrap_or_else(|| panic!("Kira definition executes: {:?}", session.diagnostics()));
    // 1.0 < 2.5 -> then arm (1); 5.0 < 2.5 is false -> else arm (0).
    assert_eq!(
        run(&result, "main", &[1], Backend::Vm).expect("Kira-built module runs"),
        klf_vm::Outcome::Return(Some(1))
    );
    assert_eq!(
        run(&result, "main", &[5], Backend::Vm).expect("Kira-built module runs"),
        klf_vm::Outcome::Return(Some(0))
    );
}
