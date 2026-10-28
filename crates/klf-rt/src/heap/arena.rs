use super::{ARENA_CHUNK, ARENA_MARK, Heap};
use crate::meta::{IMMORTAL, INIT};
use crate::span::{LARGE_CLASS, MAGIC, REMOTE_END, SENTINEL, SPAN_SIZE, SpanHeader, meta_ref};
use core::ptr;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::alloc::{Layout, alloc, handle_alloc_error};

impl Heap {
    #[must_use]
    pub fn arena_mark(&self) -> (usize, usize) {
        (self.arena_chunk, self.arena_offset)
    }

    pub fn arena_release(&mut self, mark: (usize, usize)) {
        self.arena_chunk = mark.0;
        self.arena_offset = mark.1;
    }

    pub fn arena_alloc(&mut self, size: usize, align: usize) -> Option<*mut u8> {
        self.arena_alloc_with(size, align, true)
    }

    pub fn arena_alloc_raw(&mut self, size: usize, align: usize) -> Option<*mut u8> {
        self.arena_alloc_with(size, align, false)
    }

    fn arena_alloc_with(&mut self, size: usize, align: usize, zeroed: bool) -> Option<*mut u8> {
        if size > ARENA_CHUNK / 4 {
            return None;
        }
        let align = align.max(16);
        loop {
            if self.arena_chunk == self.arena.len() {
                let chunk = self.create_arena_chunk();
                self.arena.push(chunk);
            }
            let h = self.arena[self.arena_chunk];
            let hdr = unsafe { &*h };
            let start = hdr.base() + hdr.data_start as usize;
            let capacity = hdr.slot_size as usize;
            let offset = (start + self.arena_offset).next_multiple_of(align) - start;
            if offset + size <= capacity {
                self.arena_offset = offset + size;
                let pointer = (start + offset) as *mut u8;
                if zeroed {
                    unsafe { ptr::write_bytes(pointer, 0, size) };
                }
                return Some(pointer);
            }
            self.arena_chunk += 1;
            self.arena_offset = 0;
        }
    }

    fn create_arena_chunk(&mut self) -> *mut SpanHeader {
        let layout = Layout::from_size_align(ARENA_CHUNK, SPAN_SIZE).expect("valid arena layout");
        let base = unsafe { alloc(layout) };
        let base = ptr::NonNull::new(base).unwrap_or_else(|| handle_alloc_error(layout));
        let h = base.as_ptr().cast::<SpanHeader>();
        let data_start = (crate::span::HEADER_SIZE + crate::span::META_BYTES).next_multiple_of(64);
        unsafe {
            ptr::write(
                h,
                SpanHeader {
                    magic: MAGIC,
                    class_index: LARGE_CLASS,
                    _pad: ARENA_MARK,
                    slot_size: u32::try_from(ARENA_CHUNK - data_start).expect("arena size fits"),
                    slot_count: 1,
                    free_list_head: SENTINEL,
                    data_start: u32::try_from(data_start).expect("offset fits"),
                    run_pages: u32::try_from(ARENA_CHUNK / SPAN_SIZE).expect("pages fit"),
                    owning_thread_cache: self.id,
                    remote_free: AtomicU32::new(REMOTE_END),
                    owner: self.owner,
                    destroys: 0,
                    next_remote: AtomicUsize::new(0),
                },
            );
            meta_ref(h, 0).store(INIT | IMMORTAL, Ordering::Relaxed);
        }
        let first = base.as_ptr() as usize >> 16;
        for i in 0..ARENA_CHUNK / SPAN_SIZE {
            self.pages().insert(first + i, h as usize);
        }
        self.backing += layout.size();
        self.large.insert(base.as_ptr() as usize, layout);
        h
    }
}
