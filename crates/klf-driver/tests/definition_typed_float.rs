use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

const UNIT: &str = "1.25 0.5 main Pair x y";

const DEFINITION: &str = r"
construct TypedFloat() extends Language<Tree, Program> {
    function units(session: Session, inputs: [SourceFile]) -> [Unit] {
        var units: [Unit] = []
        for file in inputs { units.append(definition_unit(session, file)) }
        return units
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
        let real = kir_type_float(64)
        let pair = kir_type_record(builder, 14, 18, 0)
        kir_type_record_field(pair, 19, 20, real)
        kir_type_record_field(pair, 21, 22, word)
        let signature = kir_signature_new(builder, 9, 13, word)
        let main = kir_signature_define(signature)
        let first = kir_value_float_literal(main, real, 0, 4)
        let second = kir_value_float_literal(main, real, 5, 8)
        let sum = kir_value_float_binary(main, 0, first, second)
        let scale = kir_value_convert(main, 4, kir_value_integer(main, word, 4), real)
        let product = kir_value_float_binary(main, 2, sum, scale)
        let negative = kir_value_float_negate(main, product)
        let restored = kir_value_float_negate(main, negative)
        let integer = kir_value_convert(main, 6, restored, word)
        let size = kir_value_integer(main, word, kir_type_size(builder, pair))
        kir_function_result(main, kir_value_binary(main, 0, 1, 1, integer, size))
        return kir_module_finish(builder)
    }
}
";

#[test]
fn typed_float_services_and_layout_run_on_both_backends() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("target");
    let mut image_session = Session::new(target.clone());
    let module = KiraDefinition
        .compile(
            &mut image_session,
            "definition.klfd",
            DEFINITION,
            "Definition",
            target.clone(),
        )
        .unwrap_or_else(|| panic!("definition: {:?}", image_session.diagnostics()));
    let image = DefinitionImage::new(
        LanguageDescriptor::minimal("typed-float", &["test"]),
        module,
    )
    .expect("definition image");
    let definition =
        ExecutableDefinition::with_executor(image, BackendDefinitionExecutor::new(Backend::Vm));
    let mut session = Session::new(target.clone());
    let built = definition
        .compile(&mut session, "input.test", UNIT, "Main", target)
        .unwrap_or_else(|| panic!("execution: {:?}", session.diagnostics()));
    let diagnostics = klf_kir_verify::verify_module(&built);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    for backend in [Backend::Vm, Backend::Native] {
        assert_eq!(
            run(&built, "main", &[], backend).expect("program runs"),
            klf_vm::Outcome::Return(Some(23)),
            "{backend:?}"
        );
    }
}
