//! The builder services for the section-12 instructions close to the
//! machine (section 8.11: one builder method per instruction): a
//! definition builds a program from `select`, `fma`, the raw pointer
//! instructions, the atomics, `fence`, the layout queries and the vector
//! instructions, and both backends run it.

use klf_driver::{Backend, BackendDefinitionExecutor, run};
use klf_kira_lower::KiraDefinition;
use klf_services::{
    Definition, DefinitionImage, ExecutableDefinition, LanguageDescriptor, Session,
};

const UNIT: &str = "main";

// A raw slot of two words: 40 and 2. The program reads 40 back, measures
// the second word's distance (8), adds 5 to the second word atomically
// (reading 2), exchanges 40 for 100, loads the 100 and picks it by the
// exchange's success, stores 1 into the second word and reads it back,
// asks the size of a word (8), and fuses 2 * 3 + 1 (7):
// 100 + 8 + 2 + 1 + 8 + 7 = 126.
const DEFINITION: &str = r"
construct Machine() extends Language<Tree, Program> {
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
        let signature = kir_signature_new(builder, 0, 4, word)
        let main = kir_signature_define(signature)
        let slot = kir_value_heap_alloc_raw(main, kir_value_integer(main, word, 16), kir_value_integer(main, word, 8))
        kir_value_raw_store(main, kir_value_integer(main, word, 40), slot, 0, 0)
        let second = kir_value_pointer_offset(main, slot, kir_value_integer(main, word, 8))
        kir_value_raw_store(main, kir_value_integer(main, word, 2), second, 8, 1)
        let first = kir_value_raw_load(main, word, slot, 0, 0)
        let distance = kir_value_pointer_diff(main, second, slot)
        let before = kir_value_atomic_rmw(main, 1, second, kir_value_integer(main, word, 5), 5)
        kir_value_fence(main, 5)
        let read = kir_value_compare_exchange(main, 0, slot, first, kir_value_integer(main, word, 100), 5, 0)
        let exchanged = kir_value_compare_exchange_succeeded(main, read)
        let loaded = kir_value_atomic_load(main, word, slot, 2)
        let picked = kir_value_select(main, exchanged, loaded, read)
        kir_value_atomic_store(main, kir_value_integer(main, word, 1), second, 3)
        let stored = kir_value_raw_load(main, word, second, 8, 1)
        let size = kir_value_size_of(main, word)
        let two = kir_value_convert(main, 4, kir_value_integer(main, word, 2), real)
        let three = kir_value_convert(main, 4, kir_value_integer(main, word, 3), real)
        let one = kir_value_convert(main, 4, kir_value_integer(main, word, 1), real)
        let fused = kir_value_convert(main, 6, kir_value_fma(main, two, three, one), word)
        kir_value_heap_free_raw(main, slot)
        let a = kir_value_binary(main, 0, 1, 1, picked, distance)
        let b = kir_value_binary(main, 0, 1, 1, a, before)
        let c = kir_value_binary(main, 0, 1, 1, b, stored)
        let d = kir_value_binary(main, 0, 1, 1, c, size)
        kir_function_result(main, kir_value_binary(main, 0, 1, 1, d, fused))
        return kir_module_finish(builder)
    }
}
";

// Four lanes of 5 with lane 2 set to 9, plus four lanes of 1: 6 6 10 6.
// A shuffle takes lane 2 of the sum and lane 0 of the ones (mask lane 4):
// 10 + 1, plus lane 0 of the sum: 17.
const VECTORS: &str = r"
construct Vectors() extends Language<Tree, Program> {
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
        let four = kir_type_vector(4, word)
        let signature = kir_signature_new(builder, 0, 4, word)
        let main = kir_signature_define(signature)
        let fives = kir_value_vector_splat(main, four, kir_value_integer(main, word, 5))
        let set = kir_value_vector_insert(main, fives, 2, kir_value_integer(main, word, 9))
        let ones = kir_value_vector_splat(main, four, kir_value_integer(main, word, 1))
        let sum = kir_value_binary(main, 0, 1, 1, set, ones)
        let picked = kir_value_vector_shuffle(main, sum, ones, 2)
        kir_value_vector_shuffle_lane(main, picked, 4)
        let a = kir_value_vector_extract(main, picked, 0)
        let b = kir_value_vector_extract(main, picked, 1)
        let c = kir_value_vector_extract(main, sum, 0)
        let ab = kir_value_binary(main, 0, 1, 1, a, b)
        kir_function_result(main, kir_value_binary(main, 0, 1, 1, ab, c))
        return kir_module_finish(builder)
    }
}
";

/// Compiles `definition`, runs it over the unit, and checks that the
/// program it builds verifies and returns `expected` on both backends.
fn builds_and_runs(definition: &str, expected: u64) {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("target");
    let mut image_session = Session::new(target.clone());
    let module = KiraDefinition
        .compile(
            &mut image_session,
            "definition.klfd",
            definition,
            "Definition",
            target.clone(),
        )
        .unwrap_or_else(|| panic!("definition: {:?}", image_session.diagnostics()));
    let image = DefinitionImage::new(LanguageDescriptor::minimal("machine", &["test"]), module)
        .expect("definition image");
    let definition =
        ExecutableDefinition::with_executor(image, BackendDefinitionExecutor::new(Backend::Vm));
    let mut session = Session::new(target.clone());
    let built = definition
        .compile(&mut session, "input.test", UNIT, "Main", target)
        .unwrap_or_else(|| panic!("execution: {:?}", session.diagnostics()));
    let diagnostics = klf_kir_verify::verify_module(&built);
    assert!(diagnostics.is_empty(), "{diagnostics:?}\n{built}");
    for backend in [Backend::Vm, Backend::Native] {
        assert_eq!(
            run(&built, "main", &[], backend).expect("program runs"),
            klf_vm::Outcome::Return(Some(expected)),
            "{backend:?}"
        );
    }
}

#[test]
fn machine_instruction_services_build_a_program_both_backends_run() {
    builds_and_runs(DEFINITION, 126);
}

#[test]
fn vector_services_build_a_program_both_backends_run() {
    builds_and_runs(VECTORS, 17);
}
