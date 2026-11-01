use klf_services::{KERNEL_SERVICE_MANIFEST, KernelServiceManifest, ServiceType};

#[test]
fn native_shim_signatures_match_the_manifest() {
    let manifest = KernelServiceManifest::parse(KERNEL_SERVICE_MANIFEST).expect("manifest");
    for (symbol, parameters, result) in super::SIGNATURES {
        let service = manifest
            .services()
            .iter()
            .find(|service| service.symbol == *symbol)
            .unwrap_or_else(|| panic!("{symbol} is missing from the manifest"));
        let expected: Vec<&str> = service
            .signature
            .parameters
            .iter()
            .map(|ty| match ty {
                ServiceType::Integer {
                    bits: 64,
                    signed: true,
                } => "i64",
                ServiceType::Pointer => "*mut u8",
                other => panic!("unsupported shim parameter {other:?}"),
            })
            .collect();
        assert_eq!(*parameters, expected, "{symbol}");
        let expected_result = match service.signature.result {
            ServiceType::Integer {
                bits: 64,
                signed: true,
            } => "i64",
            ServiceType::Void => "void",
            other => panic!("unsupported shim result {other:?}"),
        };
        assert_eq!(*result, expected_result, "{symbol}");
    }
}
