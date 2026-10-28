//! Valgrind client requests, so memcheck sees each object the heap hands out
//! rather than only the spans it carves them from. Outside Valgrind every
//! request is a few no-op instructions and answers zero.

const RUNNING_ON_VALGRIND: u64 = 0x1001;
const MALLOCLIKE_BLOCK: u64 = 0x1301;
const FREELIKE_BLOCK: u64 = 0x1302;
const MAKE_MEM_NOACCESS: u64 = 0x4d43_0000;

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn request(args: [u64; 6]) -> u64 {
    let result: u64;
    // SAFETY: the rotations of x12 sum to 128 and leave it unchanged, and
    // `orr x10, x10, x10` is a no-op; under Valgrind the sequence is the
    // client request ABI, reading the six words at x4 and answering in x3.
    unsafe {
        core::arch::asm!(
            "ror x12, x12, #3",
            "ror x12, x12, #13",
            "ror x12, x12, #51",
            "ror x12, x12, #61",
            "orr x10, x10, x10",
            inout("x3") 0_u64 => result,
            in("x4") args.as_ptr(),
            options(nostack),
        );
    }
    result
}

#[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
fn request(_: [u64; 6]) -> u64 {
    0
}

pub(crate) fn running() -> bool {
    request([RUNNING_ON_VALGRIND, 0, 0, 0, 0, 0]) != 0
}

pub(crate) fn allocated(address: usize, size: usize) {
    request([MALLOCLIKE_BLOCK, address as u64, size as u64, 0, 0, 0]);
}

pub(crate) fn freed(address: usize) {
    request([FREELIKE_BLOCK, address as u64, 0, 0, 0, 0]);
}

pub(crate) fn no_access(address: usize, size: usize) {
    request([MAKE_MEM_NOACCESS, address as u64, size as u64, 0, 0, 0]);
}
