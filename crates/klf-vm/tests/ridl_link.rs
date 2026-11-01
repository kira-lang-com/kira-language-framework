//! Separate RIDL declarations retain overload identity when linked.

#[test]
fn links_ridl_declarations_without_collapsing_overloads_to_c_symbols() {
    let program = klf_kir::parse_module(r#"
module @Program lang "karc" target "x86_64-pc-windows-msvc" stage canonical
func module foreign @pick#1(%x: i32) -> i32 engine(runtime) abi(ridl)
func module foreign @pick#2(%x: i32) -> i32 engine(runtime) abi(ridl)
func module @main() -> i32 engine(runtime) {
entry():
  %x = const.int i32, 0
  %a = call @pick#1(%x)
  %b = call @pick#2(%x)
  %ten = const.int i32, 10
  %scaled = mul [wrap] s %a, %ten
  %result = add [wrap] s %scaled, %b
  return %result
}

"#).expect("program parses");
    let library = klf_kir::parse_module(r#"
module @Library lang "karc" target "x86_64-pc-windows-msvc" stage canonical
func module @pick#1(%x: i32) -> i32 engine(runtime) abi(ridl) {
entry(%x: i32):
  %one = const.int i32, 1
  return %one
}
func module @pick#2(%x: i32) -> i32 engine(runtime) abi(ridl) {
entry(%x: i32):
  %two = const.int i32, 2
  return %two
}
"#).expect("library parses");
    let mut program = klf_vm::lower_module(&program).expect("lower program");
    let library = klf_vm::lower_module(&library).expect("lower library");
    let main = program.functions.iter().position(|function| function.name.as_str() == "main").expect("main");
    klf_vm::link(&mut program, library).expect("link library");
    assert_eq!(klf_vm::run(&program, main, &[]), klf_vm::Outcome::Return(Some(12)));
}

#[test]
fn platform_imports_keep_their_c_symbol() {
    let module = klf_kir::parse_module(r#"
module @Program lang "c" target "x86_64-pc-windows-msvc" stage canonical
func module foreign @external#1(%x: i32) -> i32 engine(runtime)
func module @main() -> i32 engine(runtime) {
entry():
  %x = const.int i32, 0
  %result = call @external#1(%x)
  return %result
}
"#).expect("program parses");
    let program = klf_vm::lower_module(&module).expect("lower program");
    assert!(program.imports.iter().any(|import| import.symbol == "external"));
}
