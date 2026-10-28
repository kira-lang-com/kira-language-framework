//! Machine code an earlier build made for a KIR function, found by what its
//! lowering reads: the function itself and, through every name it holds,
//! the items of the module that name reaches. A function nothing it reads
//! changed is then neither lowered nor compiled again.
//!
//! An item's digest covers the items it names in turn, so a change to a
//! type reaches every function laying out a value that holds it. Items
//! naming each other share one digest of their whole cycle.

use crate::cache::Key;
use crate::compile::Compiled;
use cranelift::codegen::binemit::Reloc;
use cranelift::codegen::ir::{KnownSymbol, LibCall};
use cranelift::codegen::isa::TargetIsa;
use cranelift::codegen::isa::unwind::UnwindInfo;
use cranelift_module::{DataId, FuncId, FuncOrDataId, Linkage, Module, ModuleReloc, ModuleRelocTarget};
use cranelift_object::ObjectModule;
use klf_kir::{Function, Module as KirModule};
use rustc_hash::FxHashMap as HashMap;
use serde::{Serialize, Serializer, ser};
use sha2::{Digest, Sha256};

/// Each item of a module by name, with the digest of what it is.
pub(crate) struct Items<'m> {
    names: HashMap<&'m str, u32>,
    digests: Vec<Key>,
    /// What every function's key starts from: the compiler, the target and
    /// the module as a whole.
    build: Key,
}

/// One named item. A function stands here for what its callers read of it:
/// everything but its body, and the symbol it is declared under.
enum Item<'m> {
    Function(&'m Function, &'m Option<(String, Linkage)>),
    Type(&'m klf_kir::TypeDeclaration),
    Global(&'m klf_kir::GlobalDeclaration),
    Vtable(&'m klf_kir::Vtable),
    Trait(&'m klf_kir::TraitDeclaration),
}

impl<'m> Items<'m> {
    /// The items of `module`, its functions declared as `declared` says, in
    /// the same order.
    pub(crate) fn of(
        module: &'m KirModule,
        declared: &'m [Option<(String, Linkage)>],
        isa: &dyn TargetIsa,
        unwinding: bool,
    ) -> Self {
        let mut names = HashMap::default();
        let mut groups: Vec<Vec<Item<'m>>> = Vec::new();
        let mut add = |name: &'m str, item: Item<'m>| {
            let next = u32::try_from(groups.len()).unwrap_or(u32::MAX);
            let group = *names.entry(name).or_insert(next);
            if group == next {
                groups.push(Vec::new());
            }
            groups[group as usize].push(item);
        };
        for (function, declared) in module.functions.iter().zip(declared) {
            add(function.name.as_str(), Item::Function(function, declared));
        }
        for declaration in &module.types {
            add(declaration.name.as_str(), Item::Type(declaration));
        }
        for declaration in &module.globals {
            add(declaration.name.as_str(), Item::Global(declaration));
        }
        for vtable in &module.vtables {
            add(vtable.name.as_str(), Item::Vtable(vtable));
        }
        for declaration in &module.traits {
            add(declaration.name.as_str(), Item::Trait(declaration));
        }
        let shallow: Vec<(Key, Vec<u32>)> = crate::compile::in_order(
            groups.len(),
            || (),
            |(), group| shallow_digest(&groups[group], &names),
        );
        let digests = deep_digests(&shallow);
        Self {
            build: build_digest(module, isa, unwinding),
            names,
            digests,
        }
    }

    /// The key of `function`'s code, lowered counting the call depth or
    /// not; none when part of it does not hash.
    pub(crate) fn key(&self, function: &Function, counts_depth: bool) -> Option<Key> {
        let mut hashing = Hashing::new(&self.names, &["span"]);
        hashing.hasher.update(self.build);
        hashing.hasher.update([u8::from(counts_depth)]);
        function.serialize(&mut hashing).ok()?;
        let Hashing {
            mut hasher, named, ..
        } = hashing;
        for item in named {
            hasher.update(self.digests[item as usize]);
        }
        Some(hasher.finalize().into())
    }
}

fn shallow_digest(items: &[Item<'_>], names: &HashMap<&str, u32>) -> (Key, Vec<u32>) {
    let mut hashing = Hashing::new(names, &["span", "blocks"]);
    for item in items {
        // An item that does not hash differs from every other build's.
        let hashed = match item {
            Item::Function(function, declared) => {
                hashing.hasher.update([0]);
                if let Some((symbol, linkage)) = declared {
                    hashing.hasher.update(symbol.as_bytes());
                    hashing.hasher.update([0, linkage_tag(*linkage)]);
                }
                function.serialize(&mut hashing)
            }
            Item::Type(declaration) => {
                hashing.hasher.update([1]);
                declaration.serialize(&mut hashing)
            }
            Item::Global(declaration) => {
                hashing.hasher.update([2]);
                declaration.serialize(&mut hashing)
            }
            Item::Vtable(vtable) => {
                hashing.hasher.update([3]);
                vtable.serialize(&mut hashing)
            }
            Item::Trait(declaration) => {
                hashing.hasher.update([4]);
                declaration.serialize(&mut hashing)
            }
        };
        if hashed.is_err() {
            hashing.hasher.update(std::process::id().to_le_bytes());
            hashing.hasher.update(format!("{:?}", std::time::SystemTime::now()));
        }
    }
    let Hashing { hasher, named, .. } = hashing;
    (hasher.finalize().into(), named)
}

fn linkage_tag(linkage: Linkage) -> u8 {
    match linkage {
        Linkage::Import => 0,
        Linkage::Local => 1,
        Linkage::Preemptible => 2,
        Linkage::Hidden => 3,
        Linkage::Export => 4,
    }
}

/// Each item's digest of itself and of everything it reaches. The items of
/// one cycle share the digest of the whole cycle, taken in an order of their
/// own digests, so the module's order of them does not matter.
fn deep_digests(shallow: &[(Key, Vec<u32>)]) -> Vec<Key> {
    let mut digests = vec![[0; 32]; shallow.len()];
    for cycle in cycles(shallow) {
        let mut members: Vec<&Key> = cycle.iter().map(|&item| &shallow[item as usize].0).collect();
        members.sort_unstable();
        let mut reached: Vec<&Key> = cycle
            .iter()
            .flat_map(|&item| &shallow[item as usize].1)
            .filter(|named| !cycle.contains(named))
            .map(|&named| &digests[named as usize])
            .collect();
        reached.sort_unstable();
        reached.dedup();
        let mut hasher = Sha256::new();
        for digest in members.into_iter().chain([&[0xff; 32]]).chain(reached) {
            hasher.update(digest);
        }
        let whole: Key = hasher.finalize().into();
        for &item in &cycle {
            let mut hasher = Sha256::new();
            hasher.update(shallow[item as usize].0);
            hasher.update(whole);
            digests[item as usize] = hasher.finalize().into();
        }
    }
    digests
}

/// The strongly connected items, each after every item it reaches outside
/// itself (Tarjan's algorithm, without recursion).
fn cycles(shallow: &[(Key, Vec<u32>)]) -> Vec<Vec<u32>> {
    const UNSEEN: u32 = u32::MAX;
    let count = shallow.len();
    let mut order = vec![UNSEEN; count];
    let mut low = vec![0; count];
    let mut open = vec![false; count];
    let mut stack: Vec<u32> = Vec::new();
    let mut walk: Vec<(u32, usize)> = Vec::new();
    let mut next = 0;
    let mut found = Vec::new();
    // Items are named by `u32` in `shallow`, so each root's index fits one.
    for (root, named_root) in (0..count).zip(0u32..) {
        if order[root] != UNSEEN {
            continue;
        }
        order[root] = next;
        low[root] = next;
        next += 1;
        stack.push(named_root);
        open[root] = true;
        walk.push((named_root, 0));
        while let Some(&(item, edge)) = walk.last() {
            let at = item as usize;
            if let Some(&named) = shallow[at].1.get(edge) {
                if let Some(last) = walk.last_mut() {
                    last.1 += 1;
                }
                let target = named as usize;
                if order[target] == UNSEEN {
                    order[target] = next;
                    low[target] = next;
                    next += 1;
                    stack.push(named);
                    open[target] = true;
                    walk.push((named, 0));
                } else if open[target] {
                    low[at] = low[at].min(order[target]);
                }
                continue;
            }
            walk.pop();
            if let Some(&(parent, _)) = walk.last() {
                low[parent as usize] = low[parent as usize].min(low[at]);
            }
            if low[at] == order[at] {
                let mut cycle = Vec::new();
                while let Some(member) = stack.pop() {
                    open[member as usize] = false;
                    cycle.push(member);
                    if member == item {
                        break;
                    }
                }
                found.push(cycle);
            }
        }
    }
    found
}

/// The compiler running, the target it compiles for and what the module
/// says of itself.
fn build_digest(module: &KirModule, isa: &dyn TargetIsa, unwinding: bool) -> Key {
    let mut hasher = Sha256::new();
    hasher.update(b"klf kir function code 1\0");
    let compiler = std::env::current_exe().and_then(std::fs::metadata);
    if let Ok(compiler) = compiler {
        hasher.update(compiler.len().to_le_bytes());
        hasher.update(format!("{:?}", compiler.modified().ok()));
    } else {
        // A compiler that cannot be told apart from another reuses nothing.
        hasher.update(format!("{:?}", std::time::SystemTime::now()));
    }
    hasher.update(isa.triple().to_string());
    hasher.update(isa.flags().to_string());
    hasher.update(isa.isa_flags().iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    hasher.update([u8::from(unwinding)]);
    let names = HashMap::default();
    let mut hashing = Hashing::new(&names, &[]);
    let whole = (
        &module.name,
        &module.language,
        &module.target,
        &module.stage,
        &module.attributes,
    );
    if whole.serialize(&mut hashing).is_err() {
        hashing.hasher.update(format!("{:?}", std::time::SystemTime::now()));
    }
    hasher.update(hashing.hasher.finalize());
    hasher.finalize().into()
}

/// Compiled code as the cache keeps it: every symbol it relocates against
/// by name, since the ids of another build's object number them otherwise.
#[derive(serde::Serialize, serde::Deserialize)]
struct Code {
    alignment: u64,
    bytes: Vec<u8>,
    relocs: Vec<Relocation>,
    unwind: Option<UnwindInfo>,
    landing: Vec<(u32, u32)>,
    rows: Vec<crate::debug_info::Row>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Relocation {
    offset: u32,
    kind: Reloc,
    target: Target,
    addend: i64,
}

#[derive(serde::Serialize, serde::Deserialize)]
enum Target {
    Function(String),
    Data(String),
    LibCall(LibCall),
    Known(KnownSymbol),
}

/// `compiled` as the cache keeps it; none when it relocates against what
/// has no name.
pub(crate) fn remembered(object: &ObjectModule, compiled: &Compiled) -> Option<Vec<u8>> {
    let declarations = object.declarations();
    let relocs = compiled
        .relocs
        .iter()
        .map(|reloc| {
            let target = match &reloc.name {
                name @ ModuleRelocTarget::User { .. } => {
                    if cranelift_module::ModuleDeclarations::is_function(name) {
                        let declaration = declarations.get_function_decl(FuncId::from_name(name));
                        Target::Function(declaration.name.clone()?)
                    } else {
                        let declaration = declarations.get_data_decl(DataId::from_name(name));
                        Target::Data(declaration.name.clone()?)
                    }
                }
                ModuleRelocTarget::LibCall(call) => Target::LibCall(*call),
                ModuleRelocTarget::KnownSymbol(symbol) => Target::Known(*symbol),
                ModuleRelocTarget::FunctionOffset(..) => return None,
            };
            Some(Relocation {
                offset: reloc.offset,
                kind: reloc.kind,
                target,
                addend: reloc.addend,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    postcard::to_allocvec(&Code {
        alignment: compiled.alignment,
        bytes: compiled.bytes.clone(),
        relocs,
        unwind: compiled.unwind.clone(),
        landing: compiled.landing.clone(),
        rows: compiled.rows.clone(),
    })
    .ok()
}

/// The code the cache keeps as `code` under `key`, as `id` of `object`;
/// none when it does not decode or names what `object` does not declare.
pub(crate) fn recalled(object: &ObjectModule, id: FuncId, key: Key, code: &[u8]) -> Option<Compiled> {
    let code: Code = postcard::from_bytes(code).ok()?;
    let relocs = code
        .relocs
        .into_iter()
        .map(|reloc| {
            let name = match reloc.target {
                Target::Function(symbol) => match object.get_name(&symbol)? {
                    FuncOrDataId::Func(id) => ModuleRelocTarget::from(id),
                    FuncOrDataId::Data(_) => return None,
                },
                Target::Data(symbol) => match object.get_name(&symbol)? {
                    FuncOrDataId::Data(id) => ModuleRelocTarget::from(id),
                    FuncOrDataId::Func(_) => return None,
                },
                Target::LibCall(call) => ModuleRelocTarget::LibCall(call),
                Target::Known(symbol) => ModuleRelocTarget::KnownSymbol(symbol),
            };
            Some(ModuleReloc {
                offset: reloc.offset,
                kind: reloc.kind,
                name,
                addend: reloc.addend,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Compiled {
        id,
        alignment: code.alignment,
        bytes: code.bytes,
        relocs,
        reused: Some(key),
        fresh: Vec::new(),
        unwind: code.unwind,
        landing: code.landing,
        rows: code.rows,
    })
}

/// A SHA-256 of a value as serde walks it, noting every item of the module
/// a string of it names.
struct Hashing<'n> {
    hasher: Sha256,
    names: &'n HashMap<&'n str, u32>,
    named: Vec<u32>,
    /// The fields lowering does not read, or that the items reach apart.
    skipped: &'static [&'static str],
}

impl<'n> Hashing<'n> {
    fn new(names: &'n HashMap<&'n str, u32>, skipped: &'static [&'static str]) -> Self {
        Self {
            hasher: Sha256::new(),
            names,
            named: Vec::new(),
            skipped,
        }
    }

    fn length(&mut self, length: Option<usize>) -> Result<(), Unhashable> {
        let length = length.ok_or(Unhashable)?;
        self.hasher.update((length as u64).to_le_bytes());
        Ok(())
    }
}

/// A value whose serialization hides its length.
#[derive(Debug)]
struct Unhashable;

impl std::fmt::Display for Unhashable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a value of unknown length")
    }
}

impl std::error::Error for Unhashable {}

impl ser::Error for Unhashable {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Self
    }
}

macro_rules! hash_bytes {
    ($($method:ident: $ty:ty),* $(,)?) => {
        $(fn $method(self, value: $ty) -> Result<(), Unhashable> {
            self.hasher.update(value.to_le_bytes());
            Ok(())
        })*
    };
}

impl Serializer for &mut Hashing<'_> {
    type Ok = ();
    type Error = Unhashable;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    hash_bytes!(
        serialize_i8: i8, serialize_i16: i16, serialize_i32: i32, serialize_i64: i64,
        serialize_i128: i128, serialize_u8: u8, serialize_u16: u16, serialize_u32: u32,
        serialize_u64: u64, serialize_u128: u128, serialize_f32: f32, serialize_f64: f64,
    );

    fn serialize_bool(self, value: bool) -> Result<(), Unhashable> {
        self.hasher.update([u8::from(value)]);
        Ok(())
    }

    fn serialize_char(self, value: char) -> Result<(), Unhashable> {
        self.serialize_u32(u32::from(value))
    }

    fn serialize_str(self, value: &str) -> Result<(), Unhashable> {
        self.length(Some(value.len()))?;
        self.hasher.update(value.as_bytes());
        if let Some(&item) = self.names.get(value) {
            self.named.push(item);
        }
        Ok(())
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<(), Unhashable> {
        self.length(Some(value.len()))?;
        self.hasher.update(value);
        Ok(())
    }

    fn serialize_none(self) -> Result<(), Unhashable> {
        self.hasher.update([0]);
        Ok(())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<(), Unhashable> {
        self.hasher.update([1]);
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<(), Unhashable> {
        Ok(())
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Unhashable> {
        Ok(())
    }

    fn serialize_unit_variant(self, _: &'static str, index: u32, _: &'static str) -> Result<(), Unhashable> {
        self.serialize_u32(index)
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _: &'static str, value: &T) -> Result<(), Unhashable> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unhashable> {
        self.hasher.update(index.to_le_bytes());
        value.serialize(self)
    }

    fn serialize_seq(self, length: Option<usize>) -> Result<Self, Unhashable> {
        self.length(length)?;
        Ok(self)
    }

    fn serialize_tuple(self, _: usize) -> Result<Self, Unhashable> {
        Ok(self)
    }

    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, Unhashable> {
        Ok(self)
    }

    fn serialize_tuple_variant(self, _: &'static str, index: u32, _: &'static str, _: usize) -> Result<Self, Unhashable> {
        self.hasher.update(index.to_le_bytes());
        Ok(self)
    }

    fn serialize_map(self, length: Option<usize>) -> Result<Self, Unhashable> {
        self.length(length)?;
        Ok(self)
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Unhashable> {
        Ok(self)
    }

    fn serialize_struct_variant(self, _: &'static str, index: u32, _: &'static str, _: usize) -> Result<Self, Unhashable> {
        self.hasher.update(index.to_le_bytes());
        Ok(self)
    }
}

impl ser::SerializeSeq for &mut Hashing<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Unhashable> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}

impl ser::SerializeTuple for &mut Hashing<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Unhashable> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}

impl ser::SerializeTupleStruct for &mut Hashing<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Unhashable> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}

impl ser::SerializeTupleVariant for &mut Hashing<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Unhashable> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}

impl ser::SerializeMap for &mut Hashing<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Unhashable> {
        key.serialize(&mut **self)
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Unhashable> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}

impl ser::SerializeStruct for &mut Hashing<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> Result<(), Unhashable> {
        if self.skipped.contains(&key) {
            return Ok(());
        }
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for &mut Hashing<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> Result<(), Unhashable> {
        if self.skipped.contains(&key) {
            return Ok(());
        }
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}
