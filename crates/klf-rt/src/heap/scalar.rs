use super::Heap;
use core::ptr;

impl Heap {
    /// Read up to eight little-endian bytes from an in-heap address.
    #[must_use]
    #[inline(always)]
    pub fn read(&self, ptr: *const u8, bytes: u32) -> Option<u64> {
        if bytes > 8 {
            return None;
        }
        let addr = self.access_ok(ptr.cast_mut(), bytes as usize)?;
        let value = unsafe {
            match bytes {
                0 => 0,
                1 => u64::from(*(addr as *const u8)),
                2 => u64::from(u16::from_le((addr as *const u16).read_unaligned())),
                4 => u64::from(u32::from_le((addr as *const u32).read_unaligned())),
                8 => u64::from_le((addr as *const u64).read_unaligned()),
                _ => {
                    let mut buffer = [0; 8];
                    ptr::copy_nonoverlapping(
                        addr as *const u8,
                        buffer.as_mut_ptr(),
                        bytes as usize,
                    );
                    u64::from_le_bytes(buffer)
                }
            }
        };
        Some(value)
    }

    /// Write up to eight little-endian bytes to an in-heap address.
    #[inline(always)]
    pub fn write(&mut self, ptr: *mut u8, bytes: u32, value: u64) -> bool {
        if bytes > 8 {
            return false;
        }
        let Some(addr) = self.access_ok(ptr, bytes as usize) else {
            return false;
        };
        unsafe {
            match bytes {
                0 => {}
                1 => (addr as *mut u8).write(value as u8),
                2 => (addr as *mut u16).write_unaligned((value as u16).to_le()),
                4 => (addr as *mut u32).write_unaligned((value as u32).to_le()),
                8 => (addr as *mut u64).write_unaligned(value.to_le()),
                _ => {
                    let source = value.to_le_bytes();
                    ptr::copy_nonoverlapping(source.as_ptr(), addr as *mut u8, bytes as usize);
                }
            }
        }
        true
    }
}
