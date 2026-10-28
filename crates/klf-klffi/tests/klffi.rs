//! Parsing, emission, C-header generation and KIR mapping for `.klffi`.

use klf_kir::Type;
use std::path::Path;

use klf_klffi::{
    Environment, FfiItem, FfiModule, Predefined, Remark, emit, foreign_declarations, generate,
    parse,
};

const SOKOL: &str = r"@FFI.Struct { layout: c }
struct SappDesc {
    let width: I32
    let height: I32
    let title: CString
}

@FFI.Pointer { target: SappDesc, ownership: borrowed }
struct SappDescPtr

@FFI.Callback { abi: c, params: [I32, RawPtr], result: Void }
struct EventCallback

@FFI.Extern { library: sokol, symbol: sapp_run, abi: c, retains: desc }
function sappRun(desc: SappDescPtr) -> Void

@FFI.Syscall { name: write }
function sysWrite(fd: I32, buffer: RawPtr, count: USize) -> ISize

let SAPP_MAX_TOUCHPOINTS: I32 = 8
";


/// Generates a header for a Linux target, with no include directories.
fn generate_text(header: &str, library: &str) -> (FfiModule, Vec<Remark>) {
    let predefined = Predefined::for_triple("x86_64-unknown-linux-gnu").expect("a Linux target");
    let environment = Environment {
        predefined: &predefined,
        include_dirs: &[],
    };
    generate(header, Path::new("header.h"), library, environment, &mut |_| None)
        .expect("the header preprocesses")
}

#[test]
fn parses_and_round_trips() {
    let module = parse(SOKOL).expect("the section 26.1 example parses");
    let printed = emit(&module);
    let reparsed = parse(&printed).expect("emitted text reparses");
    assert_eq!(module, reparsed, "emit round-trips");
}

#[test]
fn maps_extern_to_kir_foreign_declaration() {
    let module = parse(SOKOL).expect("parses");
    let rules = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target")
        .c_layout()
        .expect("a 1.0 target has C rules");
    let declarations = foreign_declarations(&module, &rules);
    let run = declarations
        .iter()
        .find(|f| f.name.as_str() == "sapp_run")
        .expect("the extern's C symbol names the foreign declaration");
    assert!(run.foreign);
    assert_eq!(run.parameters.len(), 1);
    assert_eq!(run.parameters[0].ty, Type::Pointer);
    assert_eq!(run.result, Type::Unit);

    let write = declarations.iter().find(|f| f.name.as_str() == "sysWrite");
    assert!(
        write.is_none(),
        "only @FFI.Extern functions become foreign declarations"
    );
}

#[test]
fn generates_klffi_from_a_c_header() {
    let header = r"
// a tiny libc excerpt
#define MAX_PATH 260
extern int puts(const char *s);
size_t strlen(const char *s);
void *memcpy(void *dst, const void *src, size_t n);
int add(int a, int b);
";
    let (module, _remarks) = generate_text(header, "c");

    let puts = module
        .items
        .iter()
        .find_map(|item| match item {
            FfiItem::Function(f) if f.name == "puts" => Some(f),
            _ => None,
        })
        .expect("puts is a prototype");
    assert_eq!(puts.parameters[0].1, "CString");
    assert_eq!(puts.result, "I32");

    let strlen = module.items.iter().find_map(|item| match item {
        FfiItem::Function(f) if f.name == "strlen" => Some(f),
        _ => None,
    });
    assert!(
        strlen.is_some(),
        "prototype without `extern` still generates"
    );

    let has_constant = module.items.iter().any(
        |item| matches!(item, FfiItem::Constant(c) if c.name == "MAX_PATH" && c.value == "260"),
    );
    assert!(has_constant, "integer #define becomes a constant");

    let printed = emit(&module);
    parse(&printed).expect("generated .klffi parses");
}

#[test]
fn generates_structs_enums_and_typedefs_from_a_c_header() {
    let header = r#"
#ifdef __cplusplus
extern "C" {
#endif
typedef unsigned int demo_flags;
enum demo_kind { DEMO_NONE, DEMO_DISK = 4, DEMO_NET };
typedef struct demo_point { int x; int y; } demo_point;
struct demo_record {
    demo_flags flags;
    struct demo_point origin;
    struct demo_record *next;
    char name[16];
    unsigned int wide : 1, tall : 1;
};
typedef void (*demo_callback)(void *context);
static inline int demo_inline(void) { return 1; }
int demo_visit(struct demo_record *record, demo_callback callback, demo_point at);
#ifdef __cplusplus
}
#endif
"#;
    let (module, remarks) = generate_text(header, "demo");
    assert!(remarks.is_empty(), "nothing is skipped: {remarks:?}");
    let printed = emit(&module);
    let expected = "\
let DEMO_NONE: I32 = 0

let DEMO_DISK: I32 = 4

let DEMO_NET: I32 = 5

@FFI.Struct { layout: c }
struct demo_point {
    let x: I32
    let y: I32
}

@FFI.Array { element: I8, count: 16 }
struct I8_array_16

@FFI.Struct { layout: c }
struct demo_record {
    let flags: U32
    let origin: demo_point
    let next: demo_record_ptr
    let name: I8_array_16
    let wide: U32 @Bits(1)
    let tall: U32 @Bits(1)
}

@FFI.Pointer { target: demo_record, ownership: borrowed }
struct demo_record_ptr

@FFI.Callback { abi: c, params: [RawPtr], result: Void }
struct demo_callback

@FFI.Extern { library: demo, symbol: demo_visit, abi: c }
function demo_visit(record: demo_record_ptr, callback: demo_callback, at: demo_point) -> I32
";
    assert_eq!(printed, expected);
    parse(&printed).expect("generated .klffi parses");
}

#[test]
fn handles_and_pointers_are_named_by_their_pointee() {
    let header = "\
typedef struct VkDevice_T* VkDevice;
int make(VkDevice device, uint32_t* count, VkDevice* out, void** data, char* name, float weights[4]);
";
    let (module, remarks) = generate_text(header, "vk");
    assert!(remarks.is_empty(), "nothing is skipped: {remarks:?}");
    let printed = emit(&module);
    assert!(
        printed.contains("@FFI.Alias { target: VkDevice_T }\nstruct VkDevice_T\n"),
        "{printed}"
    );
    assert!(
        printed.contains(
            "@FFI.Pointer { target: VkDevice_T, ownership: borrowed }\nstruct VkDevice_T_ptr\n"
        ),
        "{printed}"
    );
    assert!(
        printed.contains(
            "@FFI.Pointer { target: VkDevice_T_ptr, ownership: borrowed }\nstruct VkDevice_T_ptr_ptr\n"
        ),
        "{printed}"
    );
    assert!(
        printed.contains("@FFI.Pointer { target: U32, ownership: borrowed }\nstruct uint32_t_ptr\n"),
        "{printed}"
    );
    assert!(
        printed.contains("@FFI.Pointer { target: RawPtr, ownership: borrowed }\nstruct RawPtr_ptr\n"),
        "{printed}"
    );
    assert!(
        printed.contains(
            "function make(device: VkDevice_T_ptr, count: uint32_t_ptr, out: VkDevice_T_ptr_ptr, \
             data: RawPtr_ptr, name: CString, weights: float_ptr) -> I32"
        ),
        "{printed}"
    );
    parse(&printed).expect("generated .klffi parses");
}

#[test]
fn generated_target_dependent_scalars_follow_each_targets_c() {
    let header = "long labs(long x);\nwchar_t towlower(wchar_t c);\nintptr_t offset(uintptr_t p, ptrdiff_t d);\n";
    let (module, remarks) = generate_text(header, "c");
    assert!(remarks.is_empty(), "nothing is skipped: {remarks:?}");
    let printed = emit(&module);
    let expected = "\
@FFI.Alias { target: long }
struct CLong

@FFI.Alias { target: wchar_t }
struct CWChar

@FFI.Extern { library: c, symbol: labs, abi: c }
function labs(x: CLong) -> CLong

@FFI.Extern { library: c, symbol: towlower, abi: c }
function towlower(c: CWChar) -> CWChar

@FFI.Extern { library: c, symbol: offset, abi: c }
function offset(p: USize, d: ISize) -> ISize
";
    assert_eq!(printed, expected);
    let module = parse(&printed).expect("generated .klffi parses");
    let shapes = |triple: &str| {
        let rules = triple
            .parse::<klf_target::Target>()
            .expect("valid target")
            .c_layout()
            .expect("a 1.0 target has C rules");
        foreign_declarations(&module, &rules)
            .iter()
            .map(|function| function.result.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(shapes("x86_64-pc-windows-msvc"), ["i32", "i16", "i64"]);
    assert_eq!(shapes("x86_64-unknown-linux-gnu"), ["i64", "i32", "i64"]);
    assert_eq!(shapes("wasm32-unknown-emscripten"), ["i32", "i32", "i32"]);
}
