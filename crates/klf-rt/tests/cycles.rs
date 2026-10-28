//! The debug-build cycle detector (section 18.6) on the runtime heap.

use klf_rt::Heap;

/// A trace map for a type `name` with one strong reference per field, each
/// a pointer at successive word offsets.
fn map(name: &str, fields: &[&str]) -> Vec<u8> {
    let mut bytes = (fields.len() as u32).to_le_bytes().to_vec();
    for index in 0..fields.len() {
        bytes.extend((index as u32 * 8).to_le_bytes());
    }
    for text in std::iter::once(name).chain(fields.iter().copied()) {
        bytes.extend(text.as_bytes());
        bytes.push(0);
    }
    bytes
}

fn node(heap: &mut Heap, map: &[u8], words: usize) -> *mut u8 {
    let ptr = heap.alloc_counted(words * 8, 8, None);
    unsafe { core::ptr::write_bytes(ptr, 0, words * 8) };
    heap.trace(ptr, map.as_ptr());
    ptr
}

/// Stores a strong reference to `to` in word `word` of `from`, taking the
/// count it owns.
fn link(heap: &mut Heap, from: *mut u8, word: usize, to: *mut u8) {
    heap.retain(to);
    unsafe { (from as *mut usize).add(word).write(to as usize) };
}

#[test]
fn two_node_cycle_is_reported() {
    let mut heap = Heap::new();
    let parent = map("Parent", &["child"]);
    let child = map("Child", &["parent"]);
    let a = node(&mut heap, &parent, 1);
    let b = node(&mut heap, &child, 1);
    link(&mut heap, a, 0, b);
    link(&mut heap, b, 0, a);
    heap.release(a);
    assert!(
        heap.leaked_cycles().is_empty(),
        "b is still held from outside"
    );
    heap.release(b);
    let cycles = heap.leaked_cycles();
    assert_eq!(cycles.len(), 1);
    let cycle = &cycles[0];
    assert!(
        cycle == "Parent.child -> Child.parent -> Parent.child"
            || cycle == "Child.parent -> Parent.child -> Child.parent",
        "{cycle}"
    );
}

#[test]
fn self_reference_is_reported() {
    let mut heap = Heap::new();
    let list = map("List", &["next"]);
    let a = node(&mut heap, &list, 1);
    link(&mut heap, a, 0, a);
    heap.release(a);
    assert_eq!(heap.leaked_cycles(), ["List.next -> List.next"]);
}

#[test]
fn objects_a_cycle_reaches_are_not_part_of_it() {
    let mut heap = Heap::new();
    let pair = map("Pair", &["left", "right"]);
    let leaf = map("Leaf", &[]);
    let a = node(&mut heap, &pair, 2);
    let b = node(&mut heap, &pair, 2);
    let c = node(&mut heap, &leaf, 0);
    link(&mut heap, a, 0, b);
    link(&mut heap, b, 0, a);
    link(&mut heap, a, 1, c);
    heap.release(a);
    heap.release(b);
    let cycles = heap.leaked_cycles();
    assert_eq!(cycles.len(), 1);
    assert!(!cycles[0].contains("Leaf"), "{}", cycles[0]);
    heap.release(c);
}

#[test]
fn freed_objects_leave_no_candidates() {
    let mut heap = Heap::new();
    let list = map("List", &["next"]);
    let a = node(&mut heap, &list, 1);
    heap.retain(a);
    heap.release(a);
    heap.release(a);
    assert!(heap.leaked_cycles().is_empty());
    assert_eq!(heap.live_blocks(), 0);
}
