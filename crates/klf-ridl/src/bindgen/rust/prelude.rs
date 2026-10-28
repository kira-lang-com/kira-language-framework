//! What every Rust binding module begins with: the RIDL buffer, optional,
//! result and descriptor types its items are written in (sections 21 to 23).

pub(super) const PRELUDE: &str = r#"#![allow(non_snake_case, non_camel_case_types, dead_code)]

use core::marker::PhantomData;
use core::mem::{ManuallyDrop, MaybeUninit};

/// `BufferOps` (section 21.4).
#[repr(C)]
pub struct RidlBufferOps {
    pub abi: u32,
    pub flags: u32,
    pub realloc: unsafe extern "C" fn(*mut u8, usize, usize, usize, usize) -> *mut u8,
    pub free: unsafe extern "C" fn(*mut u8, usize, usize, usize),
}

unsafe extern "C" fn ridl_rust_realloc(
    data: *mut u8,
    old: usize,
    new: usize,
    size: usize,
    align: usize,
) -> *mut u8 {
    let Ok(layout) = std::alloc::Layout::from_size_align(new * size, align) else {
        return core::ptr::null_mut();
    };
    if old * size == 0 || data.is_null() {
        return if layout.size() == 0 { align as *mut u8 } else { unsafe { std::alloc::alloc(layout) } };
    }
    let old = unsafe { std::alloc::Layout::from_size_align_unchecked(old * size, align) };
    unsafe { std::alloc::realloc(data, old, layout.size()) }
}

unsafe extern "C" fn ridl_rust_free(data: *mut u8, cap: usize, size: usize, align: usize) {
    if cap * size != 0 && !data.is_null() {
        unsafe {
            std::alloc::dealloc(data, std::alloc::Layout::from_size_align_unchecked(cap * size, align));
        }
    }
}

/// The ops of buffers Rust allocates: the global allocator, as `Vec` uses it.
pub static RIDL_RUST_BUFFER_OPS: RidlBufferOps = RidlBufferOps {
    abi: 1,
    flags: 0,
    realloc: ridl_rust_realloc,
    free: ridl_rust_free,
};

/// An owned RIDL `[T]` (section 21.4). Its owner destroys the elements and
/// frees the storage through the ops of whoever allocated it.
#[repr(C)]
pub struct RidlBuffer<T> {
    data: *mut T,
    len: usize,
    cap: usize,
    ops: *const RidlBufferOps,
    owns: PhantomData<T>,
}

/// An owned RIDL `String`: UTF-8 bytes without a terminator.
pub type RidlString = RidlBuffer<u8>;

impl<T> RidlBuffer<T> {
    /// Takes over the storage of `items`.
    pub fn from_vec(items: Vec<T>) -> Self {
        let mut items = ManuallyDrop::new(items);
        let (len, cap) = (items.len(), items.capacity());
        if cap == 0 {
            return Self { data: core::ptr::null_mut(), len: 0, cap: 0, ops: core::ptr::null(), owns: PhantomData };
        }
        Self { data: items.as_mut_ptr(), len, cap, ops: &RIDL_RUST_BUFFER_OPS, owns: PhantomData }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[T] {
        if self.len == 0 { &[] } else { unsafe { core::slice::from_raw_parts(self.data, self.len) } }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.len == 0 { &mut [] } else { unsafe { core::slice::from_raw_parts_mut(self.data, self.len) } }
    }

    /// Moves the elements into a `Vec` and frees the storage.
    pub fn into_vec(self) -> Vec<T> {
        let this = ManuallyDrop::new(self);
        let mut items = Vec::with_capacity(this.len);
        unsafe {
            core::ptr::copy_nonoverlapping(this.data, items.as_mut_ptr(), this.len);
            items.set_len(this.len);
        }
        this.free_storage();
        items
    }

    fn free_storage(&self) {
        if !self.ops.is_null() && self.cap != 0 {
            let size = core::mem::size_of::<T>();
            let align = core::mem::align_of::<T>();
            unsafe { ((*self.ops).free)(self.data.cast(), self.cap, size, align) }
        }
    }
}

impl<T> Drop for RidlBuffer<T> {
    fn drop(&mut self) {
        unsafe { core::ptr::drop_in_place(self.as_mut_slice() as *mut [T]) }
        self.free_storage();
    }
}

impl<T: Clone> Clone for RidlBuffer<T> {
    fn clone(&self) -> Self {
        Self::from_vec(self.as_slice().to_vec())
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for RidlBuffer<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.as_slice().fmt(f)
    }
}

impl<T> From<Vec<T>> for RidlBuffer<T> {
    fn from(items: Vec<T>) -> Self {
        Self::from_vec(items)
    }
}

impl RidlBuffer<u8> {
    /// The text of a `String`.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_slice()).expect("a RIDL String holds UTF-8")
    }
}

impl From<&str> for RidlBuffer<u8> {
    fn from(text: &str) -> Self {
        Self::from_vec(text.as_bytes().to_vec())
    }
}

impl From<String> for RidlBuffer<u8> {
    fn from(text: String) -> Self {
        Self::from_vec(text.into_bytes())
    }
}

impl core::fmt::Display for RidlBuffer<u8> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A RIDL `Optional<T>` without a niche (section 21.3).
#[repr(C, u8)]
#[derive(Clone, Debug)]
pub enum RidlOptional<T> {
    None,
    Some(T),
}

impl<T> RidlOptional<T> {
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::None => None,
            Self::Some(value) => Some(value),
        }
    }
}

impl<T> From<Option<T>> for RidlOptional<T> {
    fn from(value: Option<T>) -> Self {
        match value {
            None => Self::None,
            Some(value) => Self::Some(value),
        }
    }
}

/// A RIDL `Result<T, E>` (section 21.3).
#[repr(C, u8)]
#[derive(Clone, Debug)]
pub enum RidlResult<T, E> {
    Ok(T),
    Err(E),
}

impl<T, E> RidlResult<T, E> {
    pub fn into_result(self) -> Result<T, E> {
        match self {
            Self::Ok(value) => Ok(value),
            Self::Err(error) => Err(error),
        }
    }
}

impl<T, E> From<Result<T, E>> for RidlResult<T, E> {
    fn from(value: Result<T, E>) -> Self {
        match value {
            Ok(value) => Self::Ok(value),
            Err(error) => Self::Err(error),
        }
    }
}

/// A type descriptor (section 23).
#[repr(C)]
pub struct RidlDescriptor {
    pub abi: u32,
    pub flags: u32,
    pub size: usize,
    pub align: usize,
    pub copy: *const core::ffi::c_void,
    pub r#move: *const core::ffi::c_void,
    pub destroy: Option<unsafe extern "C" fn(*mut core::ffi::c_void)>,
}

unsafe impl Sync for RidlDescriptor {}

"#;
