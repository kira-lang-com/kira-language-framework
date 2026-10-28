//! A C header whose names are Kira keywords generates a Kira binding that
//! parses: a field or parameter is renamed to `<name>_value` (a field that
//! collides gets a trailing `_`), a declared function, struct or enum member
//! named for a keyword is refused with the reason, and the C symbols that
//! remain keep their names. This is the rule the Vulkan binding relies on
//! (`VkCuLaunchInfoNVX.function`).

use std::path::Path;

use klf_klffi::{Environment, Predefined, Remark, emit, generate};

const HEADER: &str = r"
typedef struct VkCuLaunchInfo {
    int function;
    int type;
    int type_value;
    int width;
} VkCuLaunchInfo;

int do_thing(int match, struct VkCuLaunchInfo *info);

int type(int first);

enum Mode {
    MODE_A = 1,
    function = 2,
    MODE_C,
};
";

fn generate_keyword_header() -> (String, Vec<Remark>) {
    let predefined = Predefined::for_triple("x86_64-unknown-linux-gnu").expect("a Linux target");
    let environment = Environment {
        predefined: &predefined,
        include_dirs: &[],
    };
    let header = Path::new("keyword_names.h");
    let (module, remarks) = generate(
        HEADER,
        header,
        "keywords",
        environment,
        &mut |_: &Path| None::<String>,
    )
    .expect("the fixture header preprocesses");
    (emit(&module), remarks)
}

#[test]
fn a_keyword_field_is_renamed_and_a_colliding_one_gets_a_suffix() {
    let (text, _) = generate_keyword_header();
    assert!(text.contains("let function_value:"), "{text}");
    assert!(!text.contains("let function:"), "{text}");
    // `type` is renamed to `type_value`, which the struct already declares later,
    // so the renamed field takes a trailing `_` only if it came second. It came
    // first, so it is `type_value`; the later declared `type_value` collides with
    // it and becomes `type_value_`.
    assert!(text.contains("let type_value:"), "{text}");
    assert!(text.contains("let type_value_:"), "{text}");
    assert!(text.contains("let width:"), "{text}");
}

#[test]
fn a_keyword_parameter_is_renamed_and_the_function_is_kept() {
    let (text, _) = generate_keyword_header();
    assert!(text.contains("match_value:"), "{text}");
    assert!(!text.contains("match:"), "{text}");
    assert!(text.contains("do_thing"), "{text}");
}

#[test]
fn a_keyword_declared_name_is_refused_with_its_reason() {
    let (text, remarks) = generate_keyword_header();
    assert!(!text.contains("function type("), "{text}");
    assert!(
        remarks
            .iter()
            .any(|remark| remark.contains("`type`") && remark.contains("Kira keyword")),
        "{remarks:?}"
    );
}

#[test]
fn a_keyword_enum_member_is_not_declared_but_the_members_after_it_keep_their_values() {
    let (text, _) = generate_keyword_header();
    assert!(!text.contains("let function:"), "{text}");
    assert!(text.contains("let MODE_A: I32 = 1"), "{text}");
    assert!(text.contains("let MODE_C: I32 = 3"), "{text}");
}

const POINTER_HEADER: &str = r"
struct Holder {
    float *values;
    unsigned int *indices;
};
";

fn generate_pointer_header(withheld: &[String]) -> String {
    let predefined = Predefined::for_triple("x86_64-unknown-linux-gnu").expect("a Linux target");
    let environment = Environment {
        predefined: &predefined,
        include_dirs: &[],
    };
    let (module, _) = klf_klffi::generate_withholding(
        POINTER_HEADER,
        Path::new("pointers.h"),
        "pointers",
        environment,
        withheld,
        &mut |_: &Path| None::<String>,
    )
    .expect("the fixture header preprocesses");
    emit(&module)
}

#[test]
fn a_pointer_type_another_header_declares_is_withheld_and_its_name_still_resolves() {
    let withheld = generate_pointer_header(&["float_ptr".to_owned()]);
    assert!(!withheld.contains("struct float_ptr"), "{withheld}");
    assert!(withheld.contains("let values: float_ptr"), "{withheld}");
    // A name not listed is still declared.
    assert!(withheld.contains("struct unsigned_int_ptr"), "{withheld}");

    let declared = generate_pointer_header(&[]);
    assert!(declared.contains("struct float_ptr"), "{declared}");
    assert!(declared.contains("struct unsigned_int_ptr"), "{declared}");
}
