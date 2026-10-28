use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

#[test]
fn compiled_kira_phase_body_copies_an_array_bearing_nested_struct() {
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
        let inner_type = kir_type_struct_new(builder, 5, 10)
        kir_type_struct_field_array_i64(inner_type, 2, 11, 16)
        let outer_type = kir_type_struct_new(builder, 17, 22)
        kir_type_struct_field_struct(outer_type, inner_type, 23, 29)
        let main_builder = kir_function_new_i64(builder, 0, 4, 1)
        let main_block = kir_function_block_new(main_builder)
        kir_function_branch(main_builder, main_block)
        kir_function_switch_block(main_builder, main_block)
        let argument = kir_function_param_i64(main_builder, 0)
        let inner = kir_value_struct_new(main_builder, inner_type)
        let cells = kir_value_array_i64(main_builder, 2)
        kir_value_array_set_i64(main_builder, cells, 0, argument)
        let zero = kir_value_const_int(main_builder, 0)
        kir_value_array_set_i64(main_builder, cells, 1, zero)
        kir_value_struct_set_array_i64(main_builder, inner, 0, cells)
        let outer = kir_value_struct_new(main_builder, outer_type)
        kir_value_struct_set_struct(main_builder, outer, 0, inner)
        let recovered_inner = kir_value_struct_get_struct(main_builder, outer, 0)
        let recovered_cells = kir_value_struct_get_array_i64(main_builder, recovered_inner, 0)
        let result = kir_value_array_get_i64(main_builder, recovered_cells, 0)
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
        .compile(
            &mut session,
            "input.calc",
            "main Inner cells Outer nested",
            "Main",
            execution_target,
        )
        .unwrap_or_else(|| panic!("Kira definition executes: {:?}", session.diagnostics()));
    assert_eq!(
        run(&result, "main", &[5], Backend::Vm).expect("Kira-built module runs"),
        klf_vm::Outcome::Return(Some(5))
    );
}

#[test]
fn compiled_kira_phase_body_copies_a_known_enum_field_in_a_struct_payload() {
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
        let state_type = kir_type_enum_new(builder, 5, 10)
        kir_type_enum_variant_i64(state_type, 11, 16)
        let inner_type = kir_type_struct_new(builder, 17, 22)
        kir_type_struct_field_enum(inner_type, state_type, 23, 28)
        let bag_type = kir_type_enum_new(builder, 29, 32)
        kir_type_enum_variant_struct(bag_type, inner_type, 33, 37)
        let main_builder = kir_function_new_i64(builder, 0, 4, 1)
        let main_block = kir_function_block_new(main_builder)
        kir_function_branch(main_builder, main_block)
        kir_function_switch_block(main_builder, main_block)
        let argument = kir_function_param_i64(main_builder, 0)
        let state = kir_value_enum_new_i64(main_builder, state_type, 0, argument)
        let inner = kir_value_struct_new(main_builder, inner_type)
        kir_value_struct_set_enum(main_builder, inner, 0, state)
        let bag = kir_value_enum_new_struct(main_builder, bag_type, 0, inner)
        let recovered_inner = kir_value_enum_payload_struct(main_builder, bag, 0)
        let recovered_state = kir_value_struct_get_enum(main_builder, recovered_inner, 0)
        let tag = kir_value_enum_tag_i64(main_builder, recovered_state)
        let payload = kir_value_enum_payload_i64(main_builder, recovered_state, 0)
        let result = kir_value_add(main_builder, tag, payload)
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
        .compile(
            &mut session,
            "input.calc",
            "main State Ready Inner state Bag wrap",
            "Main",
            execution_target,
        )
        .unwrap_or_else(|| panic!("Kira definition executes: {:?}", session.diagnostics()));
    assert_eq!(
        run(&result, "main", &[5], Backend::Vm).expect("Kira-built module runs"),
        klf_vm::Outcome::Return(Some(5))
    );
}
