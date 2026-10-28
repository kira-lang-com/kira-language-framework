use klf_driver::{Backend, BackendDefinitionExecutor, run, run_definition_phase};
use klf_services::{
    ArtifactId, Definition, DefinitionArtifact, DefinitionArtifactKind, DefinitionImage,
    DefinitionPhase, DefinitionPhaseArguments, DefinitionRuntimeArtifact, DefinitionUnit,
    ExecutableDefinition, LanguageDescriptor, Session,
};
const IMAGE: &str = r#"module @Definition lang "klfkira" target "aarch64-apple-darwin" stage raw
func module @units() -> unit engine(any) {
bb0():
  return
}
func module @parse() -> unit engine(any) {
bb0():
  return
}
func module @elaborate() -> unit engine(any) {
bb0():
  return
}
func module @lower() -> unit engine(any) {
bb0():
  return
}
"#;
const HANDLE_IMAGE: &str = r#"module @Definition lang "klfkira" target "aarch64-apple-darwin" stage raw
func private foreign @klfsvc_definition_input(%session: i64, %index: i64) -> i64 engine(native)
func private foreign @klfsvc_definition_unit(%session: i64, %file: i64) -> i64 engine(native)
func private foreign @klfsvc_definition_units_push(%session: i64, %unit: i64) -> unit engine(native)
func private foreign @klfsvc_definition_units_finish(%session: i64) -> i64 engine(native)
func module @units(%session: i64, %count: i64) -> i64 engine(any) {
bb0(%session: i64, %count: i64):
  %zero = const.int i64, 0
  %file = call.foreign @klfsvc_definition_input(%session, %zero)
  %unit = call.foreign @klfsvc_definition_unit(%session, %file)
  call.foreign @klfsvc_definition_units_push(%session, %unit)
  %list = call.foreign @klfsvc_definition_units_finish(%session)
  return %list
}
func module @parse(%session: ptr, %unit: i64) -> i64 engine(any) {
bb0(%session: ptr, %unit: i64):
  %handle = const.int i64, 0
  return %handle
}
func module @elaborate(%session: ptr, %unit: i64, %tree: i64) -> i64 engine(any) {
bb0(%session: ptr, %unit: i64, %tree: i64):
  %handle = const.int i64, 0
  return %handle
}
func module @lower(%session: ptr, %unit: i64, %program: i64) -> i64 engine(any) {
bb0(%session: ptr, %unit: i64, %program: i64):
  %handle = const.int i64, 0
  return %handle
}
"#;
const SERVICE_IMAGE: &str = r#"module @Definition lang "klfkira" target "aarch64-apple-darwin" stage raw
func private foreign @klfsvc_definition_unit(%session: i64, %file: i64) -> i64 engine(native)
func private foreign @klfsvc_definition_input(%session: i64, %index: i64) -> i64 engine(native)
func private foreign @klfsvc_definition_units_push(%session: i64, %unit: i64) -> unit engine(native)
func private foreign @klfsvc_definition_units_finish(%session: i64) -> i64 engine(native)
func private foreign @klfsvc_definition_tree(%session: i64, %unit: i64, %payload: i64) -> i64 engine(native)
func private foreign @klfsvc_definition_program(%session: i64, %unit: i64, %tree: i64) -> i64 engine(native)
func module @units(%session: i64, %count: i64) -> i64 engine(any) {
bb0(%session: i64, %count: i64):
  %zero = const.int i64, 0
  %file = call.foreign @klfsvc_definition_input(%session, %zero)
  %unit = call.foreign @klfsvc_definition_unit(%session, %file)
  call.foreign @klfsvc_definition_units_push(%session, %unit)
  %list = call.foreign @klfsvc_definition_units_finish(%session)
  return %list
}
func module @parse(%session: i64, %unit: i64) -> i64 engine(any) {
bb0(%session: i64, %unit: i64):
  %payload = const.int i64, 42
  %tree = call.foreign @klfsvc_definition_tree(%session, %unit, %payload)
  return %tree
}
func module @elaborate(%session: i64, %unit: i64, %tree: i64) -> i64 engine(any) {
bb0(%session: i64, %unit: i64, %tree: i64):
  %program = call.foreign @klfsvc_definition_program(%session, %unit, %tree)
  return %program
}
func module @lower(%session: i64, %unit: i64, %program: i64) -> i64 engine(any) {
bb0(%session: i64, %unit: i64, %program: i64):
  %module = const.int i64, 0
  return %module
}
"#;
const BUILDER_IMAGE: &str = r#"module @Definition lang "klfkira" target "aarch64-apple-darwin" stage raw
func private foreign @klfsvc_definition_unit(%session: i64, %file: i64) -> i64 engine(native)
func private foreign @klfsvc_definition_input(%session: i64, %index: i64) -> i64 engine(native)
func private foreign @klfsvc_definition_units_push(%session: i64, %unit: i64) -> unit engine(native)
func private foreign @klfsvc_definition_units_finish(%session: i64) -> i64 engine(native)
func private foreign @klfsvc_definition_tree(%session: i64, %unit: i64, %payload: i64) -> i64 engine(native)
func private foreign @klfsvc_definition_program(%session: i64, %unit: i64, %tree: i64) -> i64 engine(native)
func private foreign @klfsvc_kir_module_new(%session: i64, %unit: i64, %program: i64) -> i64 engine(native)
func private foreign @klfsvc_kir_function_new(%module: i64, %start: i64, %end: i64) -> i64 engine(native)
func private foreign @klfsvc_kir_function_return_int(%function: i64, %value: i64) -> unit engine(native)
func private foreign @klfsvc_kir_module_finish(%module: i64) -> i64 engine(native)

func module @units(%session: i64, %count: i64) -> i64 engine(any) {
bb0(%session: i64, %count: i64):
  %zero = const.int i64, 0
  %file = call.foreign @klfsvc_definition_input(%session, %zero)
  %unit = call.foreign @klfsvc_definition_unit(%session, %file)
  call.foreign @klfsvc_definition_units_push(%session, %unit)
  %list = call.foreign @klfsvc_definition_units_finish(%session)
  return %list
}
func module @parse(%session: i64, %unit: i64) -> i64 engine(any) {
bb0(%session: i64, %unit: i64):
  %payload = const.int i64, 42
  %tree = call.foreign @klfsvc_definition_tree(%session, %unit, %payload)
  return %tree
}
func module @elaborate(%session: i64, %unit: i64, %tree: i64) -> i64 engine(any) {
bb0(%session: i64, %unit: i64, %tree: i64):
  %program = call.foreign @klfsvc_definition_program(%session, %unit, %tree)
  return %program
}
func module @lower(%session: i64, %unit: i64, %program: i64) -> i64 engine(any) {
bb0(%session: i64, %unit: i64, %program: i64):
  %builder = call.foreign @klfsvc_kir_module_new(%session, %unit, %program)
  %start = const.int i64, 0
  %end = const.int i64, 4
  %function = call.foreign @klfsvc_kir_function_new(%builder, %start, %end)
  %value = const.int i64, 37
  call.foreign @klfsvc_kir_function_return_int(%function, %value)
  %module = call.foreign @klfsvc_kir_module_finish(%builder)
  return %module
}
"#;
#[test]
fn runs_a_validated_definition_phase() {
    let module = klf_kir::parse_module(IMAGE).expect("definition image parses");
    let image = DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), module)
        .expect("definition image validates");
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    let file = session.add_file("input.calc", "1 + 1");
    let unit = DefinitionUnit::new(file);
    session.register_unit(unit.clone());
    let program = DefinitionArtifact::new(&mut session, unit.clone(), ());
    let arguments = DefinitionPhaseArguments::Lower { unit, program };
    assert_eq!(
        run_definition_phase(
            &image,
            DefinitionPhase::Lower,
            &session,
            &arguments,
            Backend::Vm,
        )
        .expect("definition phase runs"),
        klf_vm::Outcome::Return(None)
    );
    assert!(
        run_definition_phase(
            &image,
            DefinitionPhase::Parse,
            &session,
            &arguments,
            Backend::Vm,
        )
        .expect_err("phase mismatch must fail before execution")
        .contains("arguments are for `lower`")
    );
}
#[test]
fn backend_executor_drives_all_typed_definition_phases() {
    let module = klf_kir::parse_module(HANDLE_IMAGE).expect("handle image parses");
    let image = DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), module)
        .expect("definition image validates");
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    let session_module = image.module().clone();
    session.store_artifact(session_module);
    let definition =
        ExecutableDefinition::with_executor(image, BackendDefinitionExecutor::new(Backend::Vm));
    let target = session.target().clone();
    let result = definition
        .compile(&mut session, "input.calc", "1 + 1", "Main", target)
        .expect("backend executor compiles");
    assert_eq!(result.name.as_str(), "Definition");
    assert_eq!(session.diagnostics(), []);
}
#[test]
fn vm_definition_services_materialize_phase_artifacts() {
    let module = klf_kir::parse_module(SERVICE_IMAGE).expect("service image parses");
    let image = DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), module)
        .expect("service image validates");
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    session.store_artifact(image.module().clone());
    let definition =
        ExecutableDefinition::with_executor(image, BackendDefinitionExecutor::new(Backend::Vm));
    let target = session.target().clone();
    let result = definition
        .compile(&mut session, "input.calc", "1 + 1", "Main", target)
        .expect("service-backed definition compiles");
    assert_eq!(result.name.as_str(), "Definition");
    assert_eq!(
        session
            .artifact::<DefinitionRuntimeArtifact>(ArtifactId::from_u32(2))
            .expect("tree artifact"),
        &DefinitionRuntimeArtifact {
            kind: DefinitionArtifactKind::Tree,
            payload: 42,
        }
    );
    assert_eq!(
        session
            .artifact::<DefinitionRuntimeArtifact>(ArtifactId::from_u32(3))
            .expect("program artifact"),
        &DefinitionRuntimeArtifact {
            kind: DefinitionArtifactKind::Program,
            payload: 2,
        }
    );
    assert_eq!(session.diagnostics(), []);
}
#[test]
fn vm_kir_builder_services_materialize_and_run_a_module() {
    let module = klf_kir::parse_module(BUILDER_IMAGE).expect("builder image parses");
    let image = DefinitionImage::new(LanguageDescriptor::minimal("calc", &["calc"]), module)
        .expect("builder image validates");
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    session.store_artifact(image.module().clone());
    let definition =
        ExecutableDefinition::with_executor(image, BackendDefinitionExecutor::new(Backend::Vm));
    let target = session.target().clone();
    let result = definition
        .compile(&mut session, "input.calc", "main", "Main", target)
        .expect("builder-backed definition compiles");
    assert_eq!(
        run(&result, "main", &[], Backend::Vm).expect("built module runs"),
        klf_vm::Outcome::Return(Some(37))
    );
}
