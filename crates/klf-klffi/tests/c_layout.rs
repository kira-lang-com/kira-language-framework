//! `CLayout` and the C record layout of section 26.2 against the C
//! toolchain: for every 1.0 target, clang's own scalar facts and its record
//! layouts (sizes, alignments and every field's bit offset) must equal what
//! the tables and the layout give.

use klf_klffi::{record_layout, type_declarations};
use klf_target::{CLayout, LongDouble, Target};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

const TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "aarch64-apple-ios",
    "aarch64-linux-android",
    "aarch64-unknown-linux-gnu",
    "aarch64-pc-windows-msvc",
    "aarch64-unknown-tessera",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "x86_64-pc-windows-gnu",
    "x86_64-unknown-tessera",
    "wasm32-unknown-unknown",
    "wasm32-unknown-emscripten",
];

const KLFFI: &str = r"@FFI.Alias { target: long }
struct CLong

@FFI.Alias { target: wchar_t }
struct WChar

@FFI.Alias { target: size_t }
struct Size

@FFI.Alias { target: CLong }
struct Again

@FFI.Array { element: U16, count: 3 }
struct U16x3

@FFI.Struct { layout: c }
struct Scalars {
    let tag: I8
    let wide: WChar
    let count: CLong
    let ratio: F64
    let name: CString
    let size: Size
    let half: I16
    let shorts: U16x3
    let flag: Bool
}

@FFI.Struct { layout: c }
struct Flags {
    let a: U8 @Bits(3)
    let b: U32 @Bits(3)
    let c: U32 @Bits(30)
    let d: U16
    let e: I64 @Bits(40)
    let f: I64 @Bits(30)
    let g: Again @Bits(5)
}

@FFI.Union { layout: c }
struct Either {
    let whole: U64
    let low: U8 @Bits(4)
    let pair: Flags
}

@FFI.Struct { layout: c }
struct Outer {
    let first: U8
    let inner: Either
    let last: U8 @Bits(1)
}
";

const C: &str = r"struct Scalars {
    signed char tag;
    __WCHAR_TYPE__ wide;
    long count;
    double ratio;
    char *name;
    __SIZE_TYPE__ size;
    short half;
    unsigned short shorts[3];
    _Bool flag;
};
struct Flags {
    unsigned char a : 3;
    unsigned int b : 3;
    unsigned int c : 30;
    unsigned short d;
    long long e : 40;
    long long f : 30;
    long g : 5;
};
union Either {
    unsigned long long whole;
    unsigned char low : 4;
    struct Flags pair;
};
struct Outer {
    unsigned char first;
    union Either inner;
    unsigned char last : 1;
};
int sizes[] = { sizeof(struct Scalars), sizeof(struct Flags), sizeof(union Either), sizeof(struct Outer) };
";

const RECORDS: &[&str] = &["Scalars", "Flags", "Either", "Outer"];

fn find_clang() -> Option<PathBuf> {
    klf_target::llvm_tool("clang")
}

fn rules(triple: &str) -> CLayout {
    triple
        .parse::<Target>()
        .expect("valid target")
        .c_layout()
        .unwrap_or_else(|| panic!("`{triple}` is a 1.0 target and has C rules"))
}

/// Compiles `source` for `triple` and fails the test with clang's message
/// when it does not compile.
fn clang(clang: &PathBuf, triple: &str, source: &str, extra: &[&str]) -> String {
    let directory = std::env::temp_dir().join(format!("klf-c-layout-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temporary directory");
    let path = directory.join(format!("{triple}.c"));
    std::fs::write(&path, source).expect("write the C source");
    let output = Command::new(clang)
        .args([
            "-target",
            &klf_target::machine_triple(triple),
            "-fsyntax-only",
        ])
        .args(extra)
        .arg(&path)
        .output()
        .expect("clang runs");
    assert!(
        output.status.success(),
        "{triple}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The static assertions that hold when `c` matches the target's C.
fn scalar_facts(c: &CLayout) -> String {
    let mut out = String::new();
    let mut fact = |ty: &str, scalar: klf_target::CScalar| {
        let _ = writeln!(
            out,
            "_Static_assert(sizeof({ty}) == {} && _Alignof({ty}) == {}, \"{ty}\");",
            scalar.size, scalar.align
        );
    };
    fact("short", c.short);
    fact("int", c.int);
    fact("long", c.long);
    fact("long long", c.long_long);
    fact("void *", c.pointer);
    fact("__SIZE_TYPE__", c.pointer);
    fact("__PTRDIFF_TYPE__", c.pointer);
    fact("__WCHAR_TYPE__", c.wchar);
    fact("float", c.float);
    fact("double", c.double);
    fact("long double", c.long_double);
    let digits = match c.long_double_format {
        LongDouble::Double => 53,
        LongDouble::X87 => 64,
        LongDouble::Quad => 113,
    };
    let _ = writeln!(
        out,
        "_Static_assert(__LDBL_MANT_DIG__ == {digits}, \"long double\");\n\
         _Static_assert(((char)-1 < 0) == {}, \"char\");\n\
         _Static_assert(((__WCHAR_TYPE__)-1 < 0) == {}, \"wchar_t\");",
        u8::from(c.char_signed),
        u8::from(c.wchar_signed)
    );
    out
}

/// Each record clang dumped: its size and alignment in bits and its field
/// offsets in bits.
fn dumped(text: &str) -> HashMap<String, (u64, u64, Vec<u64>)> {
    let mut records = HashMap::new();
    let mut name = String::new();
    let mut size = 0;
    let mut align = 0;
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("Type: ") {
            rest.rsplit(' ')
                .next()
                .unwrap_or_default()
                .clone_into(&mut name);
        } else if let Some(rest) = line.strip_prefix("Size:") {
            size = rest.parse().expect("size");
        } else if let Some(rest) = line.strip_prefix("Alignment:") {
            align = rest.parse().expect("alignment");
        } else if let Some(rest) = line.strip_prefix("FieldOffsets: [") {
            let offsets = rest
                .trim_end_matches('>')
                .trim_end_matches(']')
                .split(", ")
                .filter(|offset| !offset.is_empty())
                .map(|offset| offset.parse().expect("offset"))
                .collect();
            records.insert(name.clone(), (size, align, offsets));
        }
    }
    records
}

#[test]
fn every_target_lays_records_out_as_its_c_compiler_does() {
    let Some(compiler) = find_clang() else {
        eprintln!("skipping: no provisioned clang");
        return;
    };
    let module = klf_klffi::parse(KLFFI).expect("the module parses");
    for &triple in TARGETS {
        let c = rules(triple);
        clang(&compiler, triple, &scalar_facts(&c), &[]);
        let records = dumped(&clang(
            &compiler,
            triple,
            C,
            &["-Xclang", "-fdump-record-layouts-simple"],
        ));
        for &name in RECORDS {
            let layout = record_layout(&module, name, &c).expect("the record lays out");
            let offsets: Vec<u64> = layout
                .fields
                .iter()
                .map(|field| field.bit_offset.unwrap_or(field.offset * 8))
                .collect();
            let ours = (layout.size * 8, layout.align * 8, offsets);
            let theirs = records
                .get(name)
                .unwrap_or_else(|| panic!("clang laid out `{name}`"));
            assert_eq!(
                &ours, theirs,
                "{triple}: `{name}` (size, align, field offsets in bits)"
            );
        }
    }
}

#[test]
fn records_become_c_types_and_aliases_their_target_width() {
    let module = klf_klffi::parse(&format!(
        "{KLFFI}\n@FFI.Extern {{ library: c, symbol: labs, abi: c }}\nfunction labs(x: CLong) -> CLong\n"
    ))
    .expect("the module parses");
    let windows = rules("x86_64-pc-windows-msvc");
    let linux = rules("x86_64-unknown-linux-gnu");
    let labs = |c: &CLayout| {
        let declarations = klf_klffi::foreign_declarations(&module, c);
        let labs = declarations
            .into_iter()
            .find(|function| function.name.as_str() == "labs")
            .expect("labs is declared");
        (labs.parameters[0].ty.to_string(), labs.result.to_string())
    };
    assert_eq!(labs(&windows), ("i32".to_owned(), "i32".to_owned()));
    assert_eq!(labs(&linux), ("i64".to_owned(), "i64".to_owned()));

    let mut kir = klf_kir::parse_module(
        "module @Records lang \"c\" target \"x86_64-unknown-linux-gnu\" stage canonical\n",
    )
    .expect("the header parses");
    kir.types = type_declarations(&module, &linux).expect("the records lay out");
    let printed = kir.to_string();
    let reparsed = klf_kir::parse_module(&printed).expect("the printed module parses");
    assert_eq!(reparsed.to_string(), printed);
    let diagnostics = klf_kir_verify::verify_module(&kir);
    assert!(diagnostics.is_empty(), "{diagnostics:?}\n{printed}");
    let text: Vec<&str> = printed
        .lines()
        .filter(|line| line.starts_with("type "))
        .collect();
    assert_eq!(text.len(), 4, "{printed}");
    assert!(
        text[2].contains("explicit c")
            && text[2].contains("whole: i64 @0")
            && text[2].contains("pair: %Flags @0")
            && !text[2].contains("low"),
        "a union's fields all start at 0 and its bit-field is left out: {}",
        text[2]
    );
    assert!(text[3].ends_with("size 40 align 8"), "{}", text[3]);
    assert!(
        text[0].contains("= struct c") && !text[0].contains(" @"),
        "a struct the kernel's C rule places as C does is a plain C struct: {}",
        text[0]
    );
    assert!(
        text[1].contains("explicit c"),
        "a struct with bit-fields keeps its offsets: {}",
        text[1]
    );
}

#[test]
fn a_bit_field_wider_than_its_type_or_a_record_holding_itself_is_refused() {
    let c = rules("aarch64-apple-darwin");
    let wide =
        klf_klffi::parse("@FFI.Struct { layout: c }\nstruct Wide {\n    let x: U8 @Bits(9)\n}\n")
            .expect("parses");
    let error = record_layout(&wide, "Wide", &c).expect_err("9 bits do not fit a U8");
    assert!(error.message.contains("9 bits wide"), "{error}");
    let looped =
        klf_klffi::parse("@FFI.Struct { layout: c }\nstruct Loop {\n    let next: Loop\n}\n")
            .expect("parses");
    let error = record_layout(&looped, "Loop", &c).expect_err("a record cannot hold itself");
    assert!(error.message.contains("holds itself"), "{error}");
}
