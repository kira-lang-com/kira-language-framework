//! Whole ranges of heap memory, each checked once rather than byte by byte.

use super::Heap;
use crate::service_memory::MAX_SERVICE_BUFFER_BYTES;
use core::ptr;

impl Heap {
    /// Lets loads and stores reach memory outside the heap, which a program
    /// that calls native libraries gets back from them. Heap addresses stay
    /// checked.
    pub fn admit_native_memory(&mut self) {
        self.native_memory = true;
    }

    /// Copy a complete bounded buffer from one live heap allocation.
    #[must_use]
    pub fn read_buffer(&self, ptr: *const u8, destination: &mut [u8]) -> bool {
        if destination.len() > MAX_SERVICE_BUFFER_BYTES {
            return false;
        }
        if destination.is_empty() {
            return true;
        }
        let Some(address) = self.access_ok(ptr.cast_mut(), destination.len()) else {
            return false;
        };
        unsafe {
            ptr::copy_nonoverlapping(
                address as *const u8,
                destination.as_mut_ptr(),
                destination.len(),
            );
        }
        true
    }

    /// Copy a complete bounded buffer into one live heap allocation.
    pub fn write_buffer(&mut self, ptr: *mut u8, source: &[u8]) -> bool {
        if source.len() > MAX_SERVICE_BUFFER_BYTES {
            return false;
        }
        if source.is_empty() {
            return true;
        }
        let Some(address) = self.access_ok(ptr, source.len()) else {
            return false;
        };
        unsafe {
            ptr::copy_nonoverlapping(source.as_ptr(), address as *mut u8, source.len());
        }
        true
    }

    /// Copy `length` bytes from `source` to `destination`, both ranges inside
    /// live allocations; the ranges may overlap. Nothing is copied when
    /// either range fails its check.
    pub fn copy(&mut self, destination: *mut u8, source: *const u8, length: usize) -> bool {
        if length == 0 {
            return true;
        }
        let Some(from) = self.access_ok(source.cast_mut(), length) else {
            return false;
        };
        let Some(to) = self.access_ok(destination, length) else {
            return false;
        };
        unsafe {
            ptr::copy(from as *const u8, to as *mut u8, length);
        }
        true
    }

    /// Set `length` bytes at `destination`, a range inside one live
    /// allocation, to `byte`. Nothing is written when the range fails its
    /// check.
    pub fn fill(&mut self, destination: *mut u8, byte: u8, length: usize) -> bool {
        if length == 0 {
            return true;
        }
        let Some(to) = self.access_ok(destination, length) else {
            return false;
        };
        unsafe {
            ptr::write_bytes(to as *mut u8, byte, length);
        }
        true
    }
}
