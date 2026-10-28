//! Native entries into bytecode that the VM supplies itself (section 17.3).
//!
//! C reaches a bytecode function through its native entry, which takes the C
//! arguments, packs them into words and calls [`klfvm_invoke`]. A `.klbc`
//! carries those entries in a native library built beside it. A device that
//! lowers pushed KIR (section 17.5) has no C compiler to build that library,
//! and on some hosts it could not load one it had not signed, so the VM holds
//! a fixed pool of entries compiled into it and binds one to each function C
//! calls.
//!
//! An entry receives every argument register of the host's convention and
//! [`STACK_WORDS`] words of the stack after them, and finds each argument
//! where the slot's [`CallPlan`] places it: the same placement the VM's own
//! calls into C use. [`classed`] serves System V and AAPCS64, whose integer
//! and floating-point arguments fill separate registers each in order, with
//! one shape. [`positional`] serves Windows x64, which gives each of the
//! first four arguments the register of its position, with one shape per
//! choice of classes for those four. A stack word no argument fills lies in
//! the caller's frame, above its outgoing arguments, and is ignored. A
//! signature whose arguments reach past those words gets no entry and stays
//! unresolved.
#![allow(unsafe_code)]

use super::Instance;
use super::reentry::klfvm_invoke;
use crate::foreign::{CallPlan, Convention, Location};
use crate::program::{ForeignClass, ForeignSignature};
use std::sync::{Mutex, PoisonError};

/// How many functions C may call through VM entries at once, across every
/// instance in the process.
const SLOTS: usize = 64;
/// The words of the stack every entry reads after its registers.
const STACK_WORDS: usize = 12;

/// The entries of every slot, `$entry::<0>` to `$entry::<63>`, as a table.
macro_rules! every_slot {
    ($entry:ident) => {
        every_slot!(@ $entry:
            0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31
            32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63)
    };
    (@ $entry:ident: $($slot:literal)*) => {
        [$($entry::<$slot> as _),*]
    };
}

#[cfg(any(target_arch = "aarch64", all(target_arch = "x86_64", not(windows))))]
mod classed;
// Built on every x86_64 host under test, so the Windows convention is
// exercised where the tests run.
#[cfg(any(
    all(target_arch = "x86_64", windows),
    all(test, target_arch = "x86_64")
))]
mod positional;

/// The address of slot `slot`'s entry for `plan` under `convention`, or
/// `None` when the host has no entries of that convention.
fn entry(convention: Convention, slot: usize, plan: &CallPlan) -> Option<usize> {
    match convention {
        #[cfg(any(target_arch = "aarch64", all(target_arch = "x86_64", not(windows))))]
        Convention::Classed => Some(classed::entry(slot, plan.float_result())),
        #[cfg(any(
            all(target_arch = "x86_64", windows),
            all(test, target_arch = "x86_64")
        ))]
        Convention::Positional => Some(positional::entry(
            slot,
            plan.positional_shape(),
            plan.float_result(),
        )),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

/// What one slot's entry calls.
#[derive(Clone)]
struct Binding {
    /// The [`Instance`] the function runs on, as an address.
    instance: usize,
    /// The function's name, which finds it again in a pushed version.
    name: String,
    function: u32,
    /// Where C puts each argument.
    plan: CallPlan,
}

static BINDINGS: Mutex<Vec<Option<Binding>>> = Mutex::new(Vec::new());

/// Binds a free entry to `function` of `instance`, named `name` and called
/// with `signature`. Returns the entry's address, or `None` when the host has
/// no entries, the arguments reach past what an entry reads, or every slot
/// is in use.
pub(super) fn bind(
    instance: &Instance,
    name: &str,
    function: u32,
    signature: &ForeignSignature,
) -> Option<usize> {
    bind_as(Convention::HOST, instance, name, function, signature)
}

fn bind_as(
    convention: Convention,
    instance: &Instance,
    name: &str,
    function: u32,
    signature: &ForeignSignature,
) -> Option<usize> {
    let plan = CallPlan::with(signature, convention);
    if plan.stack_bytes > STACK_WORDS * 8 {
        return None;
    }
    let mut bindings = BINDINGS.lock().unwrap_or_else(PoisonError::into_inner);
    bindings.resize(SLOTS, None);
    let slot = bindings.iter().position(Option::is_none)?;
    let address = entry(convention, slot, &plan)?;
    bindings[slot] = Some(Binding {
        instance: std::ptr::from_ref(instance) as usize,
        name: name.to_owned(),
        function,
        plan,
    });
    Some(address)
}

/// Points every entry bound to `instance` at the index `index_of` gives its
/// function, after a push may have moved it (section 17.5: new calls enter
/// the new code). An entry whose function `index_of` does not find keeps its
/// index.
pub(super) fn retarget(instance: &Instance, index_of: impl Fn(&str) -> Option<u32>) {
    let address = std::ptr::from_ref(instance) as usize;
    let names: Vec<String> = BINDINGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .flatten()
        .filter(|binding| binding.instance == address)
        .map(|binding| binding.name.clone())
        .collect();
    // Look the functions up without the lock: an entry C calls meanwhile
    // takes it.
    let indices: Vec<(String, u32)> = names
        .into_iter()
        .filter_map(|name| index_of(&name).map(|index| (name, index)))
        .collect();
    let mut bindings = BINDINGS.lock().unwrap_or_else(PoisonError::into_inner);
    for binding in bindings.iter_mut().flatten() {
        if binding.instance != address {
            continue;
        }
        if let Some((_, index)) = indices.iter().find(|(name, _)| *name == binding.name) {
            binding.function = *index;
        }
    }
}

/// Frees every entry bound to `instance`, which is going away.
pub(super) fn release(instance: &Instance) {
    let address = std::ptr::from_ref(instance) as usize;
    let mut bindings = BINDINGS.lock().unwrap_or_else(PoisonError::into_inner);
    for slot in bindings.iter_mut() {
        if slot
            .as_ref()
            .is_some_and(|binding| binding.instance == address)
        {
            *slot = None;
        }
    }
}

/// Runs the function slot `slot` is bound to with the argument registers
/// and stack words its entry received, returning its result word.
fn run(slot: usize, integers: &[u64], floats: &[u64], stack: &[u64; STACK_WORDS]) -> u64 {
    let binding = bound(slot);
    invoke(&binding, &arguments(&binding.plan, integers, floats, stack))
}

/// One word per argument of `plan`, each read from where the plan places it.
fn arguments(plan: &CallPlan, integers: &[u64], floats: &[u64], stack: &[u64]) -> Vec<u64> {
    plan.parameters
        .iter()
        .map(|(class, location)| {
            let word = match *location {
                Location::Integer(index) => integers.get(index).copied(),
                Location::Float(index) => floats.get(index).copied(),
                // An argument packed below a word's width sits at its byte
                // offset inside that word.
                Location::Stack { offset, .. } => {
                    stack.get(offset / 8).map(|word| word >> ((offset % 8) * 8))
                }
            };
            extended(*class, word.unwrap_or(0))
        })
        .collect()
}

/// What slot `slot` is bound to. An entry C calls after its slot was freed
/// has nothing to run, which is a trap.
fn bound(slot: usize) -> Binding {
    let binding = BINDINGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(slot)
        .cloned()
        .flatten();
    let Some(binding) = binding else {
        eprint!("{}", klf_diag::report::trap_description("KLF-VM-0007").render());
        std::process::exit(1);
    };
    binding
}

/// Runs `binding`'s function with one word per parameter, returning its
/// result word.
fn invoke(binding: &Binding, words: &[u64]) -> u64 {
    let mut result = 0;
    // SAFETY: the binding names a live instance (it is released before the
    // instance drops), and `words` holds one word per parameter.
    unsafe {
        klfvm_invoke(
            binding.instance as *const Instance,
            binding.function,
            words.as_ptr(),
            &raw mut result,
        );
    }
    result
}

/// The argument word of class `class` from the register or stack word C
/// passed it in, where only the bits of its width are defined.
fn extended(class: ForeignClass, word: u64) -> u64 {
    match class {
        ForeignClass::F32 => word & 0xffff_ffff,
        ForeignClass::F64 => word,
        ForeignClass::Signed(bits) => sign_extended(word, bits),
        ForeignClass::Unsigned(bits) => zero_extended(word, bits),
    }
}

/// `word`'s low `bits` as a signed value, which is what C passed when only
/// those bits of the word are defined.
fn sign_extended(word: u64, bits: u16) -> u64 {
    match bits {
        0 | 64.. => word,
        bits => {
            let shift = 64 - u32::from(bits);
            (((word << shift) as i64) >> shift) as u64
        }
    }
}

fn zero_extended(word: u64, bits: u16) -> u64 {
    match bits {
        0 | 64.. => word,
        bits => word & ((1 << bits) - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_narrow_integer_extends_as_its_class_says() {
        assert_eq!(sign_extended(0xdead_beef_ffff_fffe, 32), u64::MAX - 1);
        assert_eq!(zero_extended(0xdead_beef_ffff_fffe, 32), 0xffff_fffe);
        assert_eq!(sign_extended(0x7f, 8), 0x7f);
        assert_eq!(zero_extended(u64::MAX, 64), u64::MAX);
    }

    /// Apple's AAPCS64 packs stack arguments by size, so two narrow ones
    /// share a word; each is found at its own offset.
    #[test]
    fn a_packed_stack_argument_is_read_at_its_offset() {
        let mut plan = CallPlan::new(&ForeignSignature {
            parameters: Vec::new(),
            result: None,
        });
        plan.parameters = vec![
            (
                ForeignClass::Unsigned(8),
                Location::Stack { offset: 0, size: 1 },
            ),
            (
                ForeignClass::Signed(16),
                Location::Stack { offset: 2, size: 2 },
            ),
            (ForeignClass::F32, Location::Stack { offset: 4, size: 4 }),
        ];
        let word = 0x3fc0_0000_fffe_0007_u64;
        assert_eq!(
            arguments(&plan, &[], &[], &[word]),
            [7, u64::MAX - 1, u64::from(1.5f32.to_bits())]
        );
    }

    #[test]
    fn a_signature_reaching_past_the_stack_words_gets_no_entry() {
        let integers = |count| {
            CallPlan::with(
                &ForeignSignature {
                    parameters: vec![ForeignClass::Unsigned(64); count],
                    result: None,
                },
                Convention::Classed,
            )
        };
        let registers = if cfg!(target_arch = "x86_64") { 6 } else { 8 };
        assert_eq!(integers(registers).stack_bytes, 0);
        assert_eq!(
            integers(registers + STACK_WORDS).stack_bytes,
            STACK_WORDS * 8
        );
        assert!(integers(registers + STACK_WORDS + 1).stack_bytes > STACK_WORDS * 8);
    }
}
