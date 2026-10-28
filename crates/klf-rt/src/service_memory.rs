/// Maximum buffer size transferred through a kernel service in one call.
pub const MAX_SERVICE_BUFFER_BYTES: usize = 16 * 1024 * 1024;

/// Bounded bulk access to memory owned by the active execution backend.
pub trait ServiceMemory {
    /// Copies one complete guest buffer into `destination`.
    fn read_buffer(&self, address: u64, destination: &mut [u8]) -> bool;

    /// Copies `source` into one complete guest buffer.
    fn write_buffer(&mut self, address: u64, source: &[u8]) -> bool;
}

/// Copies `bytes` to the guest buffer at `address`, which a definition sized
/// to `length` bytes from an earlier length query: one when it holds them
/// exactly and the copy succeeded, zero otherwise.
pub fn copy_out(memory: &mut dyn ServiceMemory, bytes: &[u8], address: u64, length: u64) -> u64 {
    if usize::try_from(length).ok() != Some(bytes.len()) {
        return 0;
    }
    u64::from(memory.write_buffer(address, bytes))
}
