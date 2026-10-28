//! Native outputs for a target this host only cross-builds.

use super::emit_binary;
use crate::toolchain::{apple_sdk, find_tool};

const IOS: &str = r#"module @Ios lang "test" target "aarch64-apple-ios" stage raw

func private @main() -> i64 engine(runtime) {
bb0():
  %r = const.int i64, 0
  return %r
}
"#;

/// Load commands of `<mach-o/loader.h>` that name an image's platform, and
/// `PLATFORM_IOS`. A deployment target older than iOS 12 is recorded with
/// the older command.
const LC_VERSION_MIN_MACOSX: u32 = 0x24;
const LC_VERSION_MIN_IPHONEOS: u32 = 0x25;
const LC_BUILD_VERSION: u32 = 0x32;
const PLATFORM_MACOS: u32 = 1;
const PLATFORM_IOS: u32 = 2;

/// The platform each platform-naming load command of a 64-bit Mach-O image
/// names.
fn platforms(image: &[u8]) -> Vec<u32> {
    let word = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().expect("four bytes"));
    assert_eq!(word(0), 0xfeed_facf, "a 64-bit Mach-O image");
    let mut platforms = Vec::new();
    let mut at = 32;
    for _ in 0..word(16) {
        match word(at) {
            LC_BUILD_VERSION => platforms.push(word(at + 8)),
            LC_VERSION_MIN_IPHONEOS => platforms.push(PLATFORM_IOS),
            LC_VERSION_MIN_MACOSX => platforms.push(PLATFORM_MACOS),
            _ => {}
        }
        at += word(at + 4) as usize;
    }
    platforms
}

#[test]
fn an_ios_program_links_as_an_ios_mach_o_executable() {
    if apple_sdk("aarch64-apple-ios").is_none() || find_tool("clang").is_none() {
        eprintln!("skipped: no iPhoneOS SDK or bundled clang");
        return;
    }
    let module = klf_kir::parse_module(IOS).expect("module parses");
    let directory = std::env::temp_dir().join(format!("klf-ios-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("scratch directory");
    let output = directory.join("app");
    emit_binary(
        module,
        &output,
        "test",
        klf_services::EntryConvention::Process,
        klf_kir_passes::Level::O0,
    )
    .unwrap_or_else(|error| panic!("the iOS program links: {error}"));
    let image = std::fs::read(&output).expect("executable written");
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(platforms(&image), [PLATFORM_IOS]);
}
