//! Linux system call numbers by `man 2` name, for `@FFI.Syscall` (section
//! 26.2). `AArch64` follows the generic table its uapi `unistd_64.h` spells;
//! x86-64 is `arch/x86/entry/syscalls/syscall_64.tbl`, its `common` and `64`
//! rows. Both are Linux 7.0's, sorted by name.

mod aarch64;
mod x86_64;

use aarch64::AARCH64;
use x86_64::X86_64;

/// The number of system call `name` on the Linux architecture `arch`
/// (`aarch64` or `x86_64`), or `None` where that architecture has none.
#[must_use]
pub fn linux_syscall(arch: &str, name: &str) -> Option<u32> {
    let table = match arch {
        "aarch64" => AARCH64,
        "x86_64" => X86_64,
        _ => return None,
    };
    table
        .binary_search_by(|(entry, _)| (*entry).cmp(name))
        .ok()
        .map(|index| table[index].1)
}

#[cfg(test)]
mod tests {
    use super::linux_syscall;

    #[test]
    fn numbers_follow_each_architecture() {
        assert_eq!(linux_syscall("aarch64", "write"), Some(64));
        assert_eq!(linux_syscall("x86_64", "write"), Some(1));
        assert_eq!(linux_syscall("x86_64", "fork"), Some(57));
        assert_eq!(linux_syscall("aarch64", "fork"), None);
        assert_eq!(linux_syscall("riscv64", "write"), None);
    }
}
