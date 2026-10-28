//! The inline fast paths for counting a `ref T` (section 16.1).
//!
//! "For `pairing: counted` types on `ref T` the backend emits inline fast paths
//! for retain and release (an atomic update at the slot metadata address
//! computed from the pointer, section 18.3) and calls the runtime only on the
//! slow path." The fast path is a compare-and-swap of the count that was
//! checked; everything else the
//! runtime does — the page-map lookup, the saturating count, the destructor,
//! the collective weak reference and the free at zero — is the slow path.
//!
//! Section 18.3 computes the metadata address as
//! `span = p & ~0xFFFF; slot = (p - span.dataStart) / span.slotSize`. That
//! holds for a pooled span, which is 64 KiB aligned and so has its own header
//! at its base. A larger allocation gets a dedicated run of spans holding one
//! slot with the header at the run base, which only the page map finds, so a
//! pointer into such a run is not `p & ~0xFFFF` and keeps calling the runtime.

use super::FnEmitter;
use klf_kir::{Pairing, Type};

/// `!(SPAN_SIZE - 1)`, which clears a pointer's offset inside its span.
const SPAN_MASK: i64 = !(64 * 1024 - 1);
/// The span header's `slot_size` and `data_start`, as offsets from its base.
const SLOT_SIZE_AT: i64 = 8;
const DATA_START_AT: i64 = 20;
/// The metadata table follows the 64-byte header, eight bytes per slot.
const META_AT: i64 = 64;
const META_BYTES: i64 = 8;
/// Section 18.2's strong count, `immortal` bit, and a saturated count.
const STRONG_MASK: i64 = 0xFFFF_FFFF;
const IMMORTAL_SHIFT: i32 = 58;
const SATURATED: i64 = -1;
/// The largest pooled slot; past it an allocation takes a dedicated run.
const LARGEST_POOLED: u64 = 16_384;

impl FnEmitter<'_, '_> {
    /// Whether `ty` is a `ref T` whose `T` is counted and whose allocation the
    /// runtime serves from one pooled span, which is where the metadata address
    /// follows from the pointer alone.
    pub(super) fn countable(&self, ty: &Type) -> bool {
        let Type::Reference(inner) = ty else {
            return false;
        };
        let Type::Nominal(name, _) = inner.as_ref() else {
            return false;
        };
        let Some(declaration) = self
            .module
            .types
            .iter()
            .find(|declaration| declaration.name == *name)
        else {
            return false;
        };
        if declaration.policy.pairing != Pairing::Counted {
            return false;
        }
        let nominal = Type::Nominal(name.clone(), Vec::new());
        let layout = klf_layout::layout_of(&nominal, &self.module.types);
        [layout.size, layout.align]
            .into_iter()
            .all(|bound| bound <= LARGEST_POOLED)
    }

    /// One strong reference taken, counted inline where the slot allows it.
    pub(super) fn retain(&mut self, pointer: &str, inline: bool) {
        if inline {
            self.count(pointer, Direction::Up);
        } else {
            self.line(&format!("  call void @klfrt_retain(ptr {pointer})"));
        }
    }

    /// One strong reference dropped, counted inline where the slot allows it.
    pub(super) fn release(&mut self, pointer: &str, inline: bool) {
        if inline {
            self.count(pointer, Direction::Down);
        } else {
            self.line(&format!("  call void @klfrt_release(ptr {pointer})"));
        }
    }

    /// Reach the slot's metadata word from the pointer and move the count there
    /// when the slot is neither frozen nor at the point the runtime must take
    /// over: null, an already-zero or saturated count, and the last reference
    /// are all the runtime's.
    fn count(&mut self, pointer: &str, direction: Direction) {
        let check = self.fresh_label("count.check");
        let retry = self.fresh_label("count.retry");
        let fast = self.fresh_label("count.fast");
        let slow = self.fresh_label("count.slow");
        let done = self.fresh_label("count.done");

        let live = self.fresh();
        self.line(&format!("  {live} = icmp ne ptr {pointer}, null"));
        self.line(&format!("  br i1 {live}, label %{check}, label %{slow}"));
        self.line(&format!("{check}:"));
        let meta = self.metadata_address(pointer);
        self.line(&format!("  br label %{retry}"));
        // The check and the update are one compare-and-swap of the value the
        // check read. A count another thread moved since then fails the swap
        // and the step runs again, so a count is never moved from a value the
        // runtime has already taken over (the last reference, or a freed slot).
        self.line(&format!("{retry}:"));
        let current = self.fresh();
        self.line(&format!(
            "  {current} = load atomic i64, ptr {meta} monotonic, align 8"
        ));
        let movable = self.movable(&current, direction);
        self.line(&format!("  br i1 {movable}, label %{fast}, label %{slow}"));
        self.line(&format!("{fast}:"));
        let (operation, delta) = direction.operation();
        let next = self.fresh();
        self.line(&format!("  {next} = {operation} i64 {current}, {delta}"));
        let swapped = self.fresh();
        self.line(&format!(
            "  {swapped} = cmpxchg ptr {meta}, i64 {current}, i64 {next} {} monotonic, align 8",
            direction.order()
        ));
        let exchanged = self.fresh();
        self.line(&format!(
            "  {exchanged} = extractvalue {{ i64, i1 }} {swapped}, 1"
        ));
        self.line(&format!(
            "  br i1 {exchanged}, label %{done}, label %{retry}"
        ));
        self.line(&format!("{slow}:"));
        let runtime = direction.runtime();
        self.line(&format!("  call void @{runtime}(ptr {pointer})"));
        self.line(&format!("  br label %{done}"));
        self.line(&format!("{done}:"));
    }

    /// `meta`, the slot's metadata word, computed the way section 18.3 does.
    fn metadata_address(&mut self, pointer: &str) -> String {
        let address = self.fresh();
        self.line(&format!("  {address} = ptrtoint ptr {pointer} to i64"));
        let span = self.fresh();
        self.line(&format!("  {span} = and i64 {address}, {SPAN_MASK}"));
        let size = self.header_field(&span, SLOT_SIZE_AT);
        let data_start = self.header_field(&span, DATA_START_AT);
        let data = self.fresh();
        self.line(&format!("  {data} = zext i32 {data_start} to i64"));
        let payload = self.fresh();
        self.line(&format!("  {payload} = add i64 {span}, {data}"));
        let inside = self.fresh();
        self.line(&format!("  {inside} = sub i64 {address}, {payload}"));
        let width = self.fresh();
        self.line(&format!("  {width} = zext i32 {size} to i64"));
        let slot = self.fresh();
        self.line(&format!("  {slot} = udiv i64 {inside}, {width}"));
        let scaled = self.fresh();
        self.line(&format!("  {scaled} = mul i64 {slot}, {META_BYTES}"));
        let offset = self.fresh();
        self.line(&format!("  {offset} = add i64 {span}, {scaled}"));
        let at = self.fresh();
        self.line(&format!("  {at} = add i64 {offset}, {META_AT}"));
        let meta = self.fresh();
        self.line(&format!("  {meta} = inttoptr i64 {at} to ptr"));
        meta
    }

    /// The 32-bit span header field `offset` bytes into the span at `span`.
    fn header_field(&mut self, span: &str, offset: i64) -> String {
        let at = self.fresh();
        self.line(&format!("  {at} = add i64 {span}, {offset}"));
        let address = self.fresh();
        self.line(&format!("  {address} = inttoptr i64 {at} to ptr"));
        let value = self.fresh();
        self.line(&format!("  {value} = load i32, ptr {address}"));
        value
    }

    /// Whether the count `current` may move inline: not frozen, not already
    /// zero, not saturated, and — dropping one — not the last reference.
    fn movable(&mut self, current: &str, direction: Direction) -> String {
        let frozen = self.fresh();
        self.line(&format!(
            "  {frozen} = lshr i64 {current}, {IMMORTAL_SHIFT}"
        ));
        let not_frozen = self.fresh();
        self.line(&format!("  {not_frozen} = icmp eq i64 {frozen}, 0"));
        let count = self.fresh();
        self.line(&format!("  {count} = and i64 {current}, {STRONG_MASK}"));
        let narrow = self.fresh();
        self.line(&format!("  {narrow} = trunc i64 {count} to i32"));
        let counted = match direction {
            Direction::Up => {
                let unsaturated = self.fresh();
                self.line(&format!(
                    "  {unsaturated} = icmp ne i32 {narrow}, {SATURATED}"
                ));
                let alive = self.fresh();
                self.line(&format!("  {alive} = icmp ne i32 {narrow}, 0"));
                let both = self.fresh();
                self.line(&format!("  {both} = and i1 {unsaturated}, {alive}"));
                both
            }
            Direction::Down => {
                let spare = self.fresh();
                self.line(&format!("  {spare} = icmp ugt i32 {narrow}, 1"));
                spare
            }
        };
        let first = self.fresh();
        self.line(&format!("  {first} = and i1 {not_frozen}, {counted}"));
        first
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Up,
    Down,
}

impl Direction {
    const fn operation(self) -> (&'static str, i64) {
        match self {
            Self::Up => ("add", 1),
            Self::Down => ("sub", 1),
        }
    }

    /// The ordering of the swap: a release publishes what this thread wrote to
    /// the object before the count can reach zero on another thread.
    const fn order(self) -> &'static str {
        match self {
            Self::Up => "monotonic",
            Self::Down => "release",
        }
    }

    const fn runtime(self) -> &'static str {
        match self {
            Self::Up => "klfrt_retain",
            Self::Down => "klfrt_release",
        }
    }
}
