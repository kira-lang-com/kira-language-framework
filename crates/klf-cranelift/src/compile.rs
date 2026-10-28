//! Machine code for lowered functions, lowered and compiled on worker
//! threads. Each function is defined in module order, so the object never
//! depends on how the work was scheduled. Code an earlier build cached is
//! taken instead of compiled again.

use crate::cache::Key;
use crate::unwind::UnwindTables;
use crate::{EmitError, err};
use cranelift::codegen::Context;
use cranelift::codegen::control::ControlPlane;
use cranelift::codegen::incremental_cache::CacheKvStore;
use cranelift::codegen::ir::Function as ClFunction;
use cranelift::codegen::isa::TargetIsa;
use cranelift::codegen::isa::unwind::UnwindInfo;
use cranelift_module::{FuncId, Module, ModuleReloc};
use cranelift_object::ObjectModule;
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) struct Compiled {
    pub(crate) id: FuncId,
    pub(crate) alignment: u64,
    pub(crate) bytes: Vec<u8>,
    pub(crate) relocs: Vec<ModuleReloc>,
    /// The cached code this was taken from.
    pub(crate) reused: Option<Key>,
    /// The code to cache for it, when it was compiled.
    pub(crate) fresh: Vec<(Key, Vec<u8>)>,
    pub(crate) unwind: Option<UnwindInfo>,
    pub(crate) landing: Vec<(u32, u32)>,
    /// Where each part of the code comes from in the source.
    pub(crate) rows: Vec<crate::debug_info::Row>,
}

/// What a build took from the cache and what it compiled for it.
#[derive(Default)]
pub(crate) struct CacheUse {
    pub(crate) reused: HashSet<Key>,
    pub(crate) fresh: Vec<(Key, Vec<u8>)>,
}

/// `make` for every position below `count`, spread over one worker per
/// available core, each taking the next position in turn with a state of its
/// own from `start`. The results come back in position order, so the object
/// never depends on how the work was scheduled.
pub(crate) fn in_order<S, R: Send>(
    count: usize,
    start: impl Fn() -> S + Sync,
    make: impl Fn(&mut S, usize) -> R + Sync,
) -> Vec<R> {
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(count.max(1));
    let next = AtomicUsize::new(0);
    let mut results: Vec<(usize, R)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut state = start();
                    let mut taken = Vec::new();
                    loop {
                        let position = next.fetch_add(1, Ordering::Relaxed);
                        if position >= count {
                            break taken;
                        }
                        taken.push((position, make(&mut state, position)));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
            .collect()
    });
    results.sort_unstable_by_key(|(position, _)| *position);
    results.into_iter().map(|(_, result)| result).collect()
}

/// Defines `compiled` in `object`, in order, adding each function's unwind
/// entry to `tables` and what it took from and gave the cache to `cache`.
pub(crate) fn define_all(
    object: &mut ObjectModule,
    compiled: Vec<(String, Compiled)>,
    tables: &mut Option<UnwindTables>,
    debug: &mut crate::debug_info::DebugTables,
    cache: &mut CacheUse,
) -> Result<(), EmitError> {
    for (name, compiled) in compiled {
        object
            .define_function_bytes(
                compiled.id,
                compiled.alignment,
                &compiled.bytes,
                &compiled.relocs,
            )
            .map_err(|e| err(format!("define `{name}`: {e}")))?;
        debug.add(compiled.id, &name, compiled.bytes.len(), compiled.rows);
        if let Some(tables) = tables {
            tables.add(compiled.id, compiled.unwind, compiled.landing);
        }
        cache.reused.extend(compiled.reused);
        cache.fresh.extend(compiled.fresh);
    }
    Ok(())
}

/// What one function takes from, and gives, the cache of earlier builds.
struct Store<'a> {
    earlier: &'a HashMap<Key, Vec<u8>>,
    reused: Cell<Option<Key>>,
    fresh: Option<(Key, Vec<u8>)>,
}

impl CacheKvStore for Store<'_> {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        let key = Key::try_from(key).ok()?;
        let code = self.earlier.get(&key)?;
        self.reused.set(Some(key));
        Some(Cow::Borrowed(code))
    }

    fn insert(&mut self, key: &[u8], code: Vec<u8>) {
        if let Ok(key) = Key::try_from(key) {
            self.fresh = Some((key, code));
        }
    }
}

pub(crate) fn compile(
    isa: &dyn TargetIsa,
    earlier: Option<&HashMap<Key, Vec<u8>>>,
    unwinding: bool,
    context: &mut Context,
    id: FuncId,
    function: ClFunction,
    places: &[klf_kir::DebugLocation],
) -> Result<Compiled, String> {
    context.clear();
    context.func = function;
    let (reused, fresh) = if let Some(earlier) = earlier {
        let mut store = Store {
            earlier,
            reused: Cell::new(None),
            fresh: None,
        };
        context
            .compile_with_cache(isa, &mut store, &mut ControlPlane::default())
            .map_err(|e| e.inner.to_string())?;
        (store.reused.get(), store.fresh)
    } else {
        context
            .compile(isa, &mut ControlPlane::default())
            .map_err(|e| e.inner.to_string())?;
        (None, None)
    };
    let compiled = context
        .compiled_code()
        .ok_or_else(|| "no compiled code".to_owned())?;
    let unwind = if unwinding {
        compiled
            .create_unwind_info(isa)
            .map_err(|e| format!("unwind info: {e}"))?
    } else {
        None
    };
    let rows = crate::debug_info::rows(compiled, places);
    Ok(Compiled {
        id,
        unwind,
        rows,
        landing: crate::unwind::landing(compiled),
        alignment: u64::from(compiled.buffer.alignment),
        bytes: compiled.buffer.data().to_vec(),
        relocs: compiled
            .buffer
            .relocs()
            .iter()
            .map(|reloc| ModuleReloc::from_mach_reloc(reloc, &context.func, id))
            .collect(),
        reused,
        fresh: fresh.into_iter().collect(),
    })
}

