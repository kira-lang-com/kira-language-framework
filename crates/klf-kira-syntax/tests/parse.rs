//! Parser tests over the public `parse` API (Appendix A.1).

use klf_core::FileId;
use klf_kira_syntax::ast::{DeclKind, ExprKind, PatternKind, StmtKind};
use klf_kira_syntax::parse;
use klf_source::SourceFile;

fn file(source: &str) -> SourceFile {
    SourceFile::new(FileId::from_u32(0), "t.klfd", source)
}

fn parse_clean(source: &str) -> klf_kira_syntax::ast::File {
    let parsed = parse(&file(source));
    assert!(
        parsed.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        parsed.diagnostics
    );
    parsed.file
}

#[test]
fn function_with_generics_and_params() {
    let ast =
        parse_clean("function id<T: Eq>(borrow mut x: T, label y: Int = 0) -> T { return x }");
    let DeclKind::Function(func) = &ast.decls[0].kind else {
        panic!("expected function");
    };
    assert_eq!(func.generics.len(), 1);
    assert_eq!(func.params.len(), 2);
    assert!(func.ret.is_some());
    assert!(func.body.is_some());
}

#[test]
fn construct_extends_language() {
    let ast = parse_clean(
        "construct Calc() extends Language<Tree, Program> {\n  let name: String = \"calc\"\n  function units(session: Session, inputs: [SourceFile]) -> [Unit] { return [] }\n}",
    );
    assert!(matches!(ast.decls[0].kind, DeclKind::Construct(_)));
}

#[test]
fn enum_with_cases_and_method() {
    let ast = parse_clean(
        "enum Expr {\n  Num(Int)\n  Add(Expr, Expr)\n  function eval() -> Int { return 0 }\n}",
    );
    let DeclKind::Enum(decl) = &ast.decls[0].kind else {
        panic!("expected enum");
    };
    assert_eq!(decl.items.len(), 3);
}

#[test]
fn precedence_binds_multiply_over_add() {
    let ast = parse_clean("function f() -> Int { return 1 + 2 * 3 }");
    let DeclKind::Function(func) = &ast.decls[0].kind else {
        panic!("expected function");
    };
    let StmtKind::Return(Some(expr)) = &func.body.as_ref().unwrap().stmts[0].kind else {
        panic!("expected return");
    };
    let ExprKind::Binary { op, rhs, .. } = &expr.kind else {
        panic!("expected top-level binary, got {:?}", expr.kind);
    };
    assert_eq!(*op, klf_kira_syntax::ast::BinOp::Add);
    assert!(matches!(
        rhs.kind,
        ExprKind::Binary {
            op: klf_kira_syntax::ast::BinOp::Mul,
            ..
        }
    ));
}

#[test]
fn parses_integer_range() {
    let ast = parse_clean("function f(n: Int) { for i in 0..n { return } }");
    let DeclKind::Function(func) = &ast.decls[0].kind else {
        panic!("expected function");
    };
    let StmtKind::For { iter, .. } = &func.body.as_ref().unwrap().stmts[0].kind else {
        panic!("expected for");
    };
    assert!(matches!(iter.kind, ExprKind::Range { .. }));
}

#[test]
fn match_is_exhaustive_arrow_arms() {
    let ast = parse_clean(
        "function f(e: Expr) {\n  match e {\n    Num(n) -> return\n    Add(a, b) -> { return }\n  }\n}",
    );
    let DeclKind::Function(func) = &ast.decls[0].kind else {
        panic!("expected function");
    };
    let StmtKind::Match { arms, .. } = &func.body.as_ref().unwrap().stmts[0].kind else {
        panic!("expected match");
    };
    assert_eq!(arms.len(), 2);
    let PatternKind::Case { bindings, .. } = &arms[1].pattern.kind else {
        panic!("expected a case pattern");
    };
    assert_eq!(bindings.len(), 2);
}

#[test]
fn struct_literal_outside_condition() {
    let ast = parse_clean("function f() -> Point { return Point { x = 1, y = 2 } }");
    let DeclKind::Function(func) = &ast.decls[0].kind else {
        panic!("expected function");
    };
    let StmtKind::Return(Some(expr)) = &func.body.as_ref().unwrap().stmts[0].kind else {
        panic!("expected return");
    };
    assert!(matches!(expr.kind, ExprKind::StructLit { .. }));
}

#[test]
fn brace_after_condition_is_the_body_not_a_struct_literal() {
    // `if p { ... }` must read `p` as the condition and `{ ... }` as the body,
    // never as a struct literal `p { ... }`.
    let ast = parse_clean("function f(p: Bool) {\n  if p { return }\n}");
    let DeclKind::Function(func) = &ast.decls[0].kind else {
        panic!("expected function");
    };
    assert!(matches!(
        func.body.as_ref().unwrap().stmts[0].kind,
        StmtKind::If { .. }
    ));
}

#[test]
fn closure_with_params_and_bare_block() {
    let ast = parse_clean("function f() { let g = { a, b in return }\n let h = { return } }");
    let DeclKind::Function(func) = &ast.decls[0].kind else {
        panic!("expected function");
    };
    let body = func.body.as_ref().unwrap();
    let StmtKind::Let { value: g, .. } = &body.stmts[0].kind else {
        panic!("expected let g");
    };
    let ExprKind::Closure { params, .. } = &g.kind else {
        panic!("expected closure");
    };
    assert_eq!(params.len(), 2);
}

#[test]
fn recovers_after_a_bad_declaration() {
    let parsed = parse(&file("function () {}\nfunction good() {}"));
    assert_ne!(parsed.diagnostics, []);
    // The good declaration after the malformed one is still parsed.
    assert!(parsed.file.decls.iter().any(|d| matches!(
        &d.kind,
        DeclKind::Function(f) if f.name.name.as_str() == "good"
    )));
}

#[test]
fn a_line_beginning_with_a_binary_operator_continues_the_previous_line() {
    let ast = parse_clean(
        "function f(a: Bool, b: Bool) -> Bool {\n    return a\n        || b\n        && a\n}",
    );
    let DeclKind::Function(func) = &ast.decls[0].kind else {
        panic!("expected function");
    };
    assert_eq!(func.body.as_ref().unwrap().stmts.len(), 1);
}

#[test]
fn match_arms_name_texts_and_else() {
    let ast = parse_clean(
        "function f(s: String) -> Int {\n  match s {\n    \"a\",\n    \"b\" -> if true { return 1 }\n    else -> return 0\n  }\n}",
    );
    let DeclKind::Function(func) = &ast.decls[0].kind else {
        panic!("expected function");
    };
    let StmtKind::Match { arms, .. } = &func.body.as_ref().unwrap().stmts[0].kind else {
        panic!("expected match");
    };
    assert_eq!(
        arms[0].pattern.kind,
        PatternKind::Text(vec!["a".to_owned(), "b".to_owned()])
    );
    assert_eq!(arms[1].pattern.kind, PatternKind::Otherwise);
}
