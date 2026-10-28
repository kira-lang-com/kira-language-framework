//! Shared heaps across threads (section 18.1): a pointer one thread's heap
//! allocated is retained, released and freed from another, and the slot
//! returns to its owner's free list the next time the owner allocates.

use crate::Heap;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

static REMOTE_DESTROYS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn remote_destroy(_p: *mut u8) {
    REMOTE_DESTROYS.fetch_add(1, Ordering::SeqCst);
}

/// A pointer handed to another thread, as a program's own code hands one.
struct Sent(usize);

#[test]
fn release_on_another_thread_destroys_and_frees_to_the_owner() {
    REMOTE_DESTROYS.store(0, Ordering::SeqCst);
    let mut owner = Heap::shared();
    let p = owner.alloc_counted(64, 8, Some(remote_destroy));
    owner.retain(p);
    let sent = Sent(p as usize);
    std::thread::spawn(move || {
        let mut other = Heap::shared();
        let p = sent.0 as *mut u8;
        other.release(p);
        assert_eq!(REMOTE_DESTROYS.load(Ordering::SeqCst), 0, "one count is left");
        other.release(p);
        assert_eq!(REMOTE_DESTROYS.load(Ordering::SeqCst), 1, "the last release destroys");
        other.orphan();
    })
    .join()
    .expect("the releasing thread finishes");
    assert_eq!(owner.live_blocks(), 1, "the owner has not collected yet");
    owner.collect_remote();
    assert_eq!(owner.live_blocks(), 0);
    let again = owner.alloc_counted(64, 8, None);
    assert_eq!(again, p, "the freed slot is reused first");
    owner.release(again);
}

#[test]
fn raw_free_on_another_thread_and_double_free_is_refused() {
    let mut owner = Heap::shared();
    let p = owner.alloc_raw(32, 8);
    let sent = Sent(p as usize);
    let (refused, answer) = mpsc::channel();
    std::thread::spawn(move || {
        let mut other = Heap::shared();
        let p = sent.0 as *mut u8;
        let first = other.free_raw(p);
        let second = other.free_raw(p);
        refused.send((first, second)).expect("the test listens");
        other.orphan();
    })
    .join()
    .expect("the freeing thread finishes");
    assert_eq!(answer.recv().expect("an answer"), (true, false));
    owner.collect_remote();
    assert_eq!(owner.live_blocks(), 0);
}

#[test]
fn an_orphaned_heap_takes_frees_after_its_thread_ends() {
    let (sent, received) = mpsc::channel();
    std::thread::spawn(move || {
        let mut ended = Heap::shared();
        let p = ended.alloc_counted(200, 8, None);
        let large = ended.alloc_counted(100_000, 8, None);
        sent.send((Sent(p as usize), Sent(large as usize))).expect("the test listens");
        ended.orphan();
    })
    .join()
    .expect("the allocating thread finishes");
    let (p, large) = received.recv().expect("two pointers");
    let mut here = Heap::shared();
    assert_eq!(here.strong_count(p.0 as *mut u8), Some(1));
    here.release(p.0 as *mut u8);
    here.release(large.0 as *mut u8);
    assert_eq!(here.strong_count(p.0 as *mut u8), Some(0), "freed into the orphan");
}

#[test]
fn many_threads_free_what_each_other_allocated() {
    const THREADS: usize = 8;
    const ROUNDS: usize = 2000;
    let (senders, receivers): (Vec<_>, Vec<_>) =
        (0..THREADS).map(|_| mpsc::channel::<Sent>()).unzip();
    let workers: Vec<_> = receivers
        .into_iter()
        .enumerate()
        .map(|(index, received)| {
            let next = senders[(index + 1) % THREADS].clone();
            std::thread::spawn(move || {
                let mut heap = Heap::shared();
                for round in 0..ROUNDS {
                    let p = heap.alloc_counted(16 + (round % 7) * 24, 8, None);
                    unsafe { p.cast::<usize>().write(round) };
                    next.send(Sent(p as usize)).expect("the neighbour listens");
                    let theirs = received.recv().expect("the other neighbour sends");
                    assert_eq!(unsafe { (theirs.0 as *const usize).read() }, round);
                    heap.release(theirs.0 as *mut u8);
                }
                // Each neighbour has released all this thread allocated
                // once both have finished their rounds.
                heap
            })
        })
        .collect();
    drop(senders);
    let mut heaps: Vec<Heap> = workers
        .into_iter()
        .map(|worker| worker.join().expect("a worker finishes"))
        .collect();
    for heap in &mut heaps {
        heap.collect_remote();
        assert_eq!(heap.live_blocks(), 0);
    }
    for heap in heaps {
        heap.orphan();
    }
}
