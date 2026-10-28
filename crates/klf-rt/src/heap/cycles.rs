//! The cycle detector of debug builds (section 18.6). A counted object the
//! compiler gave a trace map names the offsets of the strong references in
//! its payload. A release that leaves such an object alive makes it a
//! candidate root, and a collection runs synchronous trial deletion (Bacon
//! and Rajan) from the candidates. It never frees: it reports each group of
//! objects only references among themselves keep alive.
//!
//! A trace map is one block of bytes, so a compiler can emit it as data with
//! no relocations: a little-endian `u32` count, that many `u32` payload
//! offsets, then the type's name and each offset's field name, every one
//! ending in a zero byte.

use super::{AddressMap, Heap};
use crate::meta::{IMMORTAL, strong};
use crate::span::meta_ref;
use core::sync::atomic::Ordering;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Cycles {
    /// The trace map of each traced object, by payload address.
    traces: AddressMap<usize>,
    /// Traced objects a release left alive.
    candidates: AddressMap<()>,
    /// Where each traced object that has one was allocated.
    sites: AddressMap<Box<str>>,
}

impl Cycles {
    pub(super) fn released(&mut self, payload: usize) {
        if !self.traces.is_empty() && self.traces.contains_key(&payload) {
            self.candidates.insert(payload, ());
        }
    }

    pub(super) fn forget(&mut self, payload: usize) {
        if !self.traces.is_empty() {
            self.traces.remove(&payload);
            self.candidates.remove(&payload);
            self.sites.remove(&payload);
        }
    }
}

/// A trace map read back: the type's name and each strong reference's
/// offset and field name.
struct Map<'a> {
    name: &'a str,
    edges: Vec<(usize, &'a str)>,
}

/// Reads the trace map at `address`.
///
/// # Safety
/// `address` must point at a well-formed trace map that outlives the result.
unsafe fn read_map<'a>(address: usize) -> Map<'a> {
    let word = |at: usize| unsafe { core::ptr::read_unaligned((address + at) as *const u32) };
    let count = word(0) as usize;
    let mut cursor = address + 4 + count * 4;
    let mut text = || {
        let start = cursor;
        while unsafe { *(cursor as *const u8) } != 0 {
            cursor += 1;
        }
        let bytes = unsafe { core::slice::from_raw_parts(start as *const u8, cursor - start) };
        cursor += 1;
        core::str::from_utf8(bytes).unwrap_or("?")
    };
    let name = text();
    let edges = (0..count)
        .map(|index| (word(4 + index * 4) as usize, text()))
        .collect();
    Map { name, edges }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Colour {
    Gray,
    Black,
    White,
}

/// One object of a collection: its trial count, colour, and the objects its
/// strong references reach, each with the field that holds it.
struct Node<'a> {
    count: i64,
    colour: Colour,
    name: &'a str,
    children: Vec<(usize, &'a str)>,
}

impl Heap {
    /// Gives the counted object `ptr` points into the trace map at `map`,
    /// which must outlive the object.
    pub fn trace(&mut self, ptr: *mut u8, map: *const u8) {
        if let Some((_, _, payload)) = self.resolve(ptr) {
            self.cycles.traces.insert(payload, map as usize);
        }
    }

    /// Records where the traced object `ptr` points into was allocated.
    pub fn trace_site(&mut self, ptr: *mut u8, site: &str) {
        if let Some((_, _, payload)) = self.resolve(ptr)
            && self.cycles.traces.contains_key(&payload)
        {
            self.cycles.sites.insert(payload, site.into());
        }
    }

    /// Every group of traced objects that only references among themselves
    /// keep alive, each as one cycle through it written `Type.field ->
    /// Type.field -> ...`, back to where it starts, followed by the
    /// allocation sites of its objects when they were recorded.
    #[must_use]
    pub fn leaked_cycles(&self) -> Vec<String> {
        let mut nodes: HashMap<usize, Node<'_>> = HashMap::new();
        let roots: Vec<usize> = self.cycles.candidates.keys().copied().collect();
        for &root in &roots {
            self.mark_gray(root, &mut nodes);
        }
        for &root in &roots {
            scan(root, &mut nodes);
        }
        let mut reported: AddressMap<()> = AddressMap::default();
        let mut cycles = Vec::new();
        let mut white: Vec<usize> = nodes
            .iter()
            .filter(|(_, node)| node.colour == Colour::White)
            .map(|(&address, _)| address)
            .collect();
        white.sort_unstable();
        for start in white {
            if reported.contains_key(&start) {
                continue;
            }
            if let Some(path) = cycle_from(start, &nodes) {
                for &(address, _) in &path {
                    reported.insert(address, ());
                }
                cycles.push(spell(&path, &nodes, &self.cycles.sites));
            }
        }
        cycles
    }

    /// The live, mortal counted object `ptr` points into, as its payload
    /// address and strong count.
    fn counted(&self, ptr: usize) -> Option<(usize, u32)> {
        let (h, slot, payload) = self.resolve(ptr as *mut u8)?;
        let state = unsafe { meta_ref(h, slot) }.load(Ordering::Relaxed);
        (state & IMMORTAL == 0 && strong(state) != 0).then(|| (payload, strong(state)))
    }

    /// Takes one count from each object a reference of the graph under
    /// `root` reaches, as if every object of that graph were released.
    fn mark_gray<'a>(&'a self, root: usize, nodes: &mut HashMap<usize, Node<'a>>) {
        let Some((root, count)) = self.counted(root) else {
            return;
        };
        if nodes.contains_key(&root) {
            return;
        }
        let mut stack = vec![root];
        nodes.insert(root, self.node(root, count));
        while let Some(address) = stack.pop() {
            let children = nodes[&address].children.clone();
            for (child, _) in children {
                if let Some(node) = nodes.get_mut(&child) {
                    node.count -= 1;
                    continue;
                }
                let Some((_, count)) = self.counted(child) else {
                    continue;
                };
                let mut node = self.node(child, count);
                node.count -= 1;
                nodes.insert(child, node);
                stack.push(child);
            }
        }
    }

    fn node(&self, payload: usize, count: u32) -> Node<'_> {
        let Some(&map) = self.cycles.traces.get(&payload) else {
            return Node {
                count: i64::from(count),
                colour: Colour::Gray,
                name: "?",
                children: Vec::new(),
            };
        };
        let map = unsafe { read_map(map) };
        let children = map
            .edges
            .iter()
            .filter_map(|&(offset, field)| {
                let target =
                    unsafe { core::ptr::read_unaligned((payload + offset) as *const usize) };
                let (child, _) = self.counted(target)?;
                Some((child, field))
            })
            .collect();
        Node {
            count: i64::from(count),
            colour: Colour::Gray,
            name: map.name,
            children,
        }
    }
}

/// Colours white each gray object no reference from outside the graph keeps
/// alive, and black, with what it reaches, each one some reference does.
fn scan(root: usize, nodes: &mut HashMap<usize, Node<'_>>) {
    let mut stack = vec![root];
    while let Some(address) = stack.pop() {
        let Some(node) = nodes.get_mut(&address) else {
            continue;
        };
        if node.colour != Colour::Gray {
            continue;
        }
        if node.count > 0 {
            scan_black(address, nodes);
            continue;
        }
        node.colour = Colour::White;
        stack.extend(node.children.iter().map(|&(child, _)| child));
    }
}

/// Gives back the counts gray marking took from what `root` reaches.
fn scan_black(root: usize, nodes: &mut HashMap<usize, Node<'_>>) {
    nodes.get_mut(&root).expect("a marked object").colour = Colour::Black;
    let mut stack = vec![root];
    while let Some(address) = stack.pop() {
        let children = nodes[&address].children.clone();
        for (child, _) in children {
            let Some(node) = nodes.get_mut(&child) else {
                continue;
            };
            node.count += 1;
            if node.colour != Colour::Black {
                node.colour = Colour::Black;
                stack.push(child);
            }
        }
    }
}

/// A cycle of white objects reached from `start`, as each object with the
/// field that leads on to the next.
fn cycle_from<'a>(start: usize, nodes: &HashMap<usize, Node<'a>>) -> Option<Vec<(usize, &'a str)>> {
    let white = |address: &usize| {
        nodes
            .get(address)
            .is_some_and(|n| n.colour == Colour::White)
    };
    let mut path: Vec<(usize, usize)> = vec![(start, 0)];
    let mut on_path: AddressMap<usize> = AddressMap::default();
    let mut done: AddressMap<()> = AddressMap::default();
    on_path.insert(start, 0);
    while let Some(&mut (address, ref mut next)) = path.last_mut() {
        let children = &nodes[&address].children;
        let Some(&(child, _)) = children.get(*next) else {
            on_path.remove(&address);
            done.insert(address, ());
            path.pop();
            continue;
        };
        *next += 1;
        if !white(&child) || done.contains_key(&child) {
            continue;
        }
        if let Some(&depth) = on_path.get(&child) {
            return Some(
                path[depth..]
                    .iter()
                    .map(|&(at, taken)| (at, nodes[&at].children[taken - 1].1))
                    .collect(),
            );
        }
        on_path.insert(child, path.len());
        path.push((child, 0));
    }
    None
}

fn spell(
    path: &[(usize, &str)],
    nodes: &HashMap<usize, Node<'_>>,
    sites: &AddressMap<Box<str>>,
) -> String {
    let mut steps: Vec<String> = path
        .iter()
        .map(|&(address, field)| format!("{}.{field}", nodes[&address].name))
        .collect();
    steps.push(steps[0].clone());
    let mut text = steps.join(" -> ");
    let allocated: Vec<&str> = path
        .iter()
        .filter_map(|(address, _)| sites.get(address).map(AsRef::as_ref))
        .collect();
    if !allocated.is_empty() {
        text.push_str(" (allocated at ");
        text.push_str(&allocated.join(", "));
        text.push(')');
    }
    text
}
