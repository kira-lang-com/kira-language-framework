//! The C preprocessor of `klf ffi gen`: directives, conditionals, includes,
//! macro expansion and the diagnostics a malformed header gets.

use std::path::{Path, PathBuf};

use klf_klffi::{Environment, HeaderError, Predefined, Preprocessed, preprocess};

const HEADER: &str = "/src/main.h";

fn linux() -> Predefined {
    Predefined::for_triple("x86_64-unknown-linux-gnu").expect("a Linux target")
}

fn run_in(
    predefined: &Predefined,
    text: &str,
    files: &[(&str, &str)],
) -> Result<Preprocessed, HeaderError> {
    let include_dirs = [PathBuf::from("/inc")];
    let environment = Environment {
        predefined,
        include_dirs: &include_dirs,
    };
    let mut read = |path: &Path| {
        files
            .iter()
            .find(|(name, _)| Path::new(name) == path)
            .map(|(_, text)| (*text).to_owned())
    };
    preprocess(text, Path::new(HEADER), environment, &mut read)
}

fn run(text: &str) -> Preprocessed {
    run_in(&linux(), text, &[]).expect("the header preprocesses")
}

fn text_of(text: &str) -> String {
    run(text).text.trim().to_owned()
}

fn error_of(text: &str) -> HeaderError {
    run_in(&linux(), text, &[]).expect_err("the header is malformed")
}

#[test]
fn object_like_macro_expands_in_code() {
    assert_eq!(text_of("#define N 4\nint a[N];\n"), "int a[4];");
}

#[test]
fn undef_stops_expansion() {
    assert_eq!(text_of("#define A 1\n#undef A\nA\n"), "A");
}

#[test]
fn self_referential_macro_expands_once() {
    assert_eq!(text_of("#define X X\nX\n"), "X");
}

#[test]
fn function_like_macro_substitutes_arguments() {
    assert_eq!(
        text_of("#define ADD(a, b) ((a) + (b))\nADD(1, x)\n"),
        "((1) + (x))"
    );
}

#[test]
fn function_like_name_without_arguments_is_left_alone() {
    assert_eq!(text_of("#define F(x) x\nint F;\n"), "int F;");
}

#[test]
fn invocation_may_span_lines() {
    assert_eq!(
        text_of("#define F(a, b) a b\nF(\n1,\n2\n)\n"),
        "1 2"
    );
}

#[test]
fn stringify_quotes_its_argument() {
    assert_eq!(
        text_of("#define S(x) #x\nS(a  +  \"b\")\n"),
        "\"a + \\\"b\\\"\""
    );
}

#[test]
fn token_paste_joins_into_one_token() {
    assert_eq!(text_of("#define CAT(a, b) a##b\nCAT(foo, 1)\n"), "foo1");
}

#[test]
fn variadic_macro_passes_the_rest_of_its_arguments() {
    assert_eq!(
        text_of("#define V(f, ...) f(__VA_ARGS__)\nV(g, 1, 2)\n"),
        "g(1, 2)"
    );
}

#[test]
fn if_evaluates_defined_and_the_operator_set() {
    let text = "#define ON 1\n\
        #if defined(ON) && ON > 0 && (2 * 3 == 6) && (7 / 2 == 3) && (7 % 4 == 3)\n\
        yes\n\
        #else\n\
        no\n\
        #endif\n";
    assert_eq!(text_of(text), "yes");
}

#[test]
fn integer_constants_in_if_follow_c_spelling() {
    let text = "#if (0x10 == 16) && (010 == 8) && (1 << 4 == 16) && (-1 < 0) && !0 && ('A' == 65) && (5u == 5L)\nok\n#endif\n";
    assert_eq!(text_of(text), "ok");
}

#[test]
fn identifiers_read_as_zero_in_if() {
    assert_eq!(text_of("#if UNDEFINED\nno\n#else\nyes\n#endif\n"), "yes");
}

#[test]
fn elif_takes_the_first_true_branch_only() {
    let text = "#if 0\na\n#elif 1\nb\n#elif 1\nc\n#else\nd\n#endif\n";
    assert_eq!(text_of(text), "b");
}

#[test]
fn ifdef_and_ifndef_test_definitions() {
    let text = "#define HAS\n#ifdef HAS\na\n#endif\n#ifndef HAS\nb\n#endif\n#ifndef MISSING\nc\n#endif\n";
    assert_eq!(text_of(text), "a\nc");
}

#[test]
fn inactive_groups_skip_directives_they_do_not_run() {
    let text = "#if 0\n#bogus directive\n#if 1\nx\n#endif\n#endif\nz\n";
    assert_eq!(text_of(text), "z");
}

#[test]
fn an_else_group_defined_inside_a_false_group_stays_off() {
    let text = "#if 0\n#if 1\nx\n#else\ny\n#endif\n#endif\nz\n";
    assert_eq!(text_of(text), "z");
}

#[test]
fn quoted_include_resolves_beside_the_including_file() {
    let files = [
        ("/src/local.h", "#define L 7\nint l = L;\n"),
    ];
    let text = "#include \"local.h\"\nint main_value;\n";
    let preprocessed = run_in(&linux(), text, &files).expect("preprocesses");
    assert_eq!(preprocessed.text.trim(), "int l = 7;\nint main_value;");
    assert!(preprocessed.remarks.is_empty());
}

#[test]
fn angle_include_resolves_along_include_directories() {
    let files = [("/inc/vk.h", "int v;\n")];
    let preprocessed = run_in(&linux(), "#include <vk.h>\n", &files).expect("preprocesses");
    assert_eq!(preprocessed.text.trim(), "int v;");
}

#[test]
fn include_guard_admits_a_header_once() {
    let files = [(
        "/inc/g.h",
        "#ifndef G\n#define G\nint g;\n#endif\n",
    )];
    let text = "#include <g.h>\n#include <g.h>\n";
    let preprocessed = run_in(&linux(), text, &files).expect("preprocesses");
    assert_eq!(preprocessed.text.trim(), "int g;");
}

#[test]
fn include_cycle_is_left_out_with_a_remark() {
    let files = [("/src/main.h", "#include \"main.h\"\nint x;\n")];
    let preprocessed = run_in(&linux(), "#include \"main.h\"\nint x;\n", &files)
        .expect("a cycle is not an error");
    assert_eq!(preprocessed.text.trim(), "int x;");
    assert_eq!(preprocessed.remarks.len(), 1, "{:?}", preprocessed.remarks);
    assert!(preprocessed.remarks[0].contains("include cycle"));
}

#[test]
fn missing_include_is_a_remark_not_an_error() {
    let preprocessed = run("#include <stdint.h>\nint y;\n");
    assert_eq!(preprocessed.text.trim(), "int y;");
    assert_eq!(preprocessed.remarks.len(), 1);
    assert!(preprocessed.remarks[0].contains("<stdint.h>"));
    assert!(preprocessed.remarks[0].starts_with("/src/main.h:1:"));
}

#[test]
fn include_name_may_come_from_a_macro() {
    let files = [("/inc/vk.h", "int v;\n")];
    let text = "#define HDR <vk.h>\n#include HDR\n";
    let preprocessed = run_in(&linux(), text, &files).expect("preprocesses");
    assert_eq!(preprocessed.text.trim(), "int v;");
}

#[test]
fn comments_and_line_splices_are_removed() {
    let text = "#define LONG 1 \\\n + 2 /* note */\nLONG // tail\n/* block\n */ end\n";
    assert_eq!(text_of(text), "1 + 2 end");
}

#[test]
fn defines_list_each_name_once_with_its_final_value() {
    let preprocessed = run("#define A 1\n#define B(x) x\n#define C \"s\"\n#define A 2\n#define D (1+1)\n#undef C\n");
    let names: Vec<(&str, &str)> = preprocessed
        .defines
        .iter()
        .map(|d| (d.name.as_str(), d.value.as_str()))
        .collect();
    assert_eq!(names, vec![("A", "2"), ("D", "(1+1)")]);
}

#[test]
fn guard_defines_appear_in_defines_once() {
    let text = "#ifndef K\n#define K 1\n#endif\n#ifndef K\n#define K 1\n#endif\n";
    let preprocessed = run(text);
    assert_eq!(preprocessed.defines.len(), 1);
    assert_eq!(preprocessed.defines[0].name, "K");
}

#[test]
fn predefined_macros_follow_the_target_triple() {
    let windows = Predefined::for_triple("x86_64-pc-windows-msvc").expect("a Windows target");
    let text = "#if defined(_WIN32) && defined(_WIN64)\nwin64\n#elif defined(__linux__)\nlinux\n#endif\n";
    let preprocessed = run_in(&windows, text, &[]).expect("preprocesses");
    assert_eq!(preprocessed.text.trim(), "win64");
    let linux_text = "#if defined(_WIN32)\nwin\n#elif defined(__linux__) && defined(__x86_64__)\nlinux\n#endif\n";
    assert_eq!(text_of(linux_text), "linux");
    assert!(Predefined::for_triple("riscv64-unknown-unknown-foo").is_none());
}

#[test]
fn malformed_directives_report_file_and_line() {
    let cases: [(&str, usize, &str); 8] = [
        ("#if 1\nint x;\n", 1, "not closed"),
        ("int x;\n#endif\n", 2, "without `#if`"),
        ("#if 1\n#else\n#else\n#endif\n", 3, "after `#else`"),
        ("#define\n", 1, "macro name"),
        ("#error stop here\n", 1, "stop here"),
        ("#if 1 / 0\n#endif\n", 1, "division by zero"),
        ("#define F(x) #y\n", 1, "not followed by a parameter"),
        ("#define F(a) a\nF(1, 2)\n", 2, "takes 1 argument"),
    ];
    for (text, line, expected) in cases {
        let error = error_of(text);
        assert_eq!(error.line, line, "line for {text:?}: {error}");
        assert!(
            error.message.contains(expected),
            "message for {text:?} is `{}`, expected `{expected}`",
            error.message
        );
        assert_eq!(error.path, PathBuf::from(HEADER));
    }
}

#[test]
fn unterminated_argument_list_is_an_error() {
    let error = error_of("#define F(x) x\nF(1\n");
    assert!(error.message.contains("unterminated argument list"));
}

#[test]
fn unknown_directive_in_active_code_is_an_error() {
    let error = error_of("#frobnicate\n");
    assert!(error.message.contains("unknown directive `#frobnicate`"));
}
