//! `klf ffi gen` over the Vulkan `vulkan_core.h`, the header preprocessing
//! exists for: `VKAPI_ATTR` and the handle macros come from `vk_platform.h`,
//! and array lengths are `#define` constants. The test reads the header from
//! the Ember checkout and skips when it is absent.

use std::path::{Path, PathBuf};

use klf_klffi::{Environment, Predefined, emit, generate};

const VULKAN_INCLUDE: &str =
    "/Users/priamc/Code/kira-projects/development/ember/NativeLibs/Vulkan/include/vulkan";

#[test]
fn vulkan_core_header_generates_its_functions_handles_and_arrays() {
    let directory = PathBuf::from(VULKAN_INCLUDE);
    let header = directory.join("vulkan_core.h");
    let Ok(text) = std::fs::read_to_string(&header) else {
        eprintln!("skipped: {} is absent", header.display());
        return;
    };
    let predefined = Predefined::for_triple("x86_64-unknown-linux-gnu").expect("a Linux target");
    let include_dirs = [directory];
    let environment = Environment {
        predefined: &predefined,
        include_dirs: &include_dirs,
    };
    let (module, remarks) = generate(&text, &header, "vulkan", environment, &mut |path: &Path| {
        std::fs::read_to_string(path).ok()
    })
    .expect("vulkan_core.h preprocesses");
    let printed = emit(&module);

    assert!(
        printed.contains("vkCreateImageView"),
        "vkCreateImageView is generated"
    );
    for remark in &remarks {
        assert!(
            !remark.contains("array length `VK_"),
            "array lengths that are #define constants are read: {remark}"
        );
        assert!(
            !remark.contains("VKAPI"),
            "VKAPI_ATTR and VKAPI_CALL expand: {remark}"
        );
        assert!(
            !remark.contains("unmapped parameter type `Vk"),
            "handle parameters map: {remark}"
        );
    }
    for name in ["VK_USE_64_BIT_PTR_DEFINES", "VK_NULL_HANDLE"] {
        let declarations = printed.matches(&format!("let {name}:")).count();
        assert!(declarations <= 1, "{name} is emitted {declarations} times");
    }
}
