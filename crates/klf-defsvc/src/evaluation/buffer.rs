use super::{Buffer, BufferResult};
use klf_core::Name;
use klf_kir::{
    ConvertKind, GlobalDeclaration, GlobalInit, Instruction, Linkage, Module, Operation, StoreMode,
    Type, TypeDeclaration,
};
use klf_vm::{EvaluationBudget, Machine, Outcome};

/// A call to a compile-time buffer function, and the instruction it replaces.
pub(super) struct BufferCall<'a> {
    pub entry: usize,
    pub arguments: &'a [u64],
    pub descriptor: &'a BufferResult,
    pub destination: &'a Name,
    pub original: &'a Instruction,
    pub prefix: String,
}

fn host_size(value: u64) -> Result<usize, String> {
    usize::try_from(value)
        .map_err(|_| "compile-time buffer exceeds the host's address space".to_owned())
}

/// The instructions that rebuild the evaluated record at the call, and the
/// constant global holding its buffer's bytes when it has a buffer.
pub(super) fn evaluate(
    module: &Module,
    machine: &mut Machine<'_>,
    budget: &mut EvaluationBudget,
    call: &BufferCall<'_>,
) -> Result<(Vec<Instruction>, Option<GlobalDeclaration>), String> {
    let result = run(module, machine, budget, call)?;
    let mut emitter = Emitter {
        prefix: &call.prefix,
        original: call.original,
        output: Vec::new(),
    };
    let mut global = None;
    let mut pointer = None;
    if let (Some(buffer), Some(bytes)) = (&call.descriptor.buffer, result.bytes) {
        let (copied, constant) = emitter.bytes(&buffer.allocator, bytes);
        pointer = Some((buffer.data_field, copied));
        global = constant;
    }
    for (index, (field, word)) in result.record.fields.iter().zip(&result.words).enumerate() {
        let value = match &pointer {
            Some((data_field, copied)) if *data_field == index => copied.clone(),
            _ => emitter.field_value(module, machine, index, &field.ty, *word)?,
        };
        emitter.store_field(call.destination, index, value)?;
    }
    Ok((emitter.output, global))
}

/// What a buffer function left in its result record: the record's fields as
/// words, and the bytes its data field points at.
struct Evaluated<'m> {
    record: &'m TypeDeclaration,
    words: Vec<u64>,
    bytes: Option<Vec<u8>>,
}

/// Runs the buffer function into a fresh result record and reads the record
/// and its buffer back out of the machine.
fn run<'m>(
    module: &'m Module,
    machine: &mut Machine<'_>,
    budget: &mut EvaluationBudget,
    call: &BufferCall<'_>,
) -> Result<Evaluated<'m>, String> {
    let descriptor = call.descriptor;
    let function = module
        .functions
        .iter()
        .find(|function| function.name == descriptor.name)
        .ok_or("missing compile-time buffer function")?;
    let Some(Type::Address(result)) = function.parameters.first().map(|parameter| &parameter.ty)
    else {
        return Err("compile-time buffer function requires an address result parameter".to_owned());
    };
    let Type::Nominal(name, _) = result.as_ref() else {
        return Err("compile-time buffer result requires a record".to_owned());
    };
    let record = module
        .types
        .iter()
        .find(|record| record.name == *name)
        .ok_or("missing compile-time result record")?;
    let layout = klf_layout::layout_of(result, &module.types);
    let slot = machine
        .evaluation_slot(host_size(layout.size)?, host_size(layout.align)?, budget)
        .map_err(|outcome| format!("compile-time result allocation: {outcome:?}"))?;
    let mut input = vec![slot];
    input.extend_from_slice(call.arguments);
    let outcome = machine.evaluate(call.entry, &input, budget);
    if !matches!(outcome, Outcome::Return(_)) {
        return Err(format!(
            "compile-time evaluation of `{name}` failed: {outcome:?}"
        ));
    }
    let words = record_words(module, machine, record, slot)?;
    let bytes = match &descriptor.buffer {
        Some(buffer) => Some(buffer_bytes(machine, budget, buffer, &words)?),
        None => None,
    };
    Ok(Evaluated {
        record,
        words,
        bytes,
    })
}

/// The bytes the record's data field points at.
fn buffer_bytes(
    machine: &Machine<'_>,
    budget: &EvaluationBudget,
    buffer: &Buffer,
    words: &[u64],
) -> Result<Vec<u8>, String> {
    let length = *words
        .get(buffer.length_field)
        .ok_or("invalid buffer length field")?;
    let data = *words
        .get(buffer.data_field)
        .ok_or("invalid buffer data field")?;
    let size = length
        .checked_mul(buffer.element_bytes)
        .filter(|size| *size <= budget.memory_bytes as u64)
        .ok_or("compile-time buffer exceeds the memory budget")?;
    let mut bytes = vec![0; host_size(size)?];
    if !bytes.is_empty() && !machine.evaluation_read(data, &mut bytes) {
        return Err("invalid compile-time buffer data".to_owned());
    }
    Ok(bytes)
}

/// Each field of the `record` at `slot`, as a word.
fn record_words(
    module: &Module,
    machine: &Machine<'_>,
    record: &TypeDeclaration,
    slot: u64,
) -> Result<Vec<u64>, String> {
    let offsets = klf_layout::field_offsets(record.name.as_str(), &module.types);
    record
        .fields
        .iter()
        .zip(&offsets)
        .map(|(field, offset)| {
            let size = klf_layout::layout_of(&field.ty, &module.types).size;
            if size > 8 {
                return Err("nested compile-time buffer fields are not yet supported".to_owned());
            }
            let mut bytes = [0; 8];
            if !machine.evaluation_read(slot + offset, &mut bytes[..host_size(size)?]) {
                return Err("invalid compile-time record field".to_owned());
            }
            Ok(u64::from_le_bytes(bytes))
        })
        .collect()
}

/// The instructions that rebuild an evaluated buffer at run time, each named
/// after the call it replaces.
struct Emitter<'a> {
    prefix: &'a str,
    original: &'a Instruction,
    output: Vec<Instruction>,
}

impl Emitter<'_> {
    /// Emits `operation` named `<prefix>_<suffix>` and answers that name.
    fn emit(&mut self, suffix: &str, operation: Operation) -> Name {
        let name = Name::new(format!("{}_{suffix}", self.prefix));
        self.push(vec![name.clone()], operation);
        name
    }

    /// Emits `operation`, which has no result.
    fn effect(&mut self, operation: Operation) {
        self.push(Vec::new(), operation);
    }

    fn push(&mut self, results: Vec<Name>, operation: Operation) {
        self.output.push(Instruction {
            results,
            operation,
            debug_location: self.original.debug_location.clone(),
            span: self.original.span,
        });
    }

    fn integer(&mut self, suffix: &str, bits: u16, value: i128) -> Name {
        self.emit(
            suffix,
            Operation::ConstInt {
                ty: Type::Integer(bits),
                value,
            },
        )
    }

    /// Allocates a buffer with `allocator` and copies `bytes` into it from a
    /// constant global, answering the buffer's pointer and that global.
    fn bytes(&mut self, allocator: &Name, bytes: Vec<u8>) -> (Name, Option<GlobalDeclaration>) {
        let count = self.integer("count", 64, bytes.len() as i128);
        let pointer = self.emit(
            "data",
            Operation::Call {
                callee: allocator.clone(),
                arguments: vec![count.clone()],
                generic_args: Vec::new(),
            },
        );
        if bytes.is_empty() {
            return (pointer, None);
        }
        let name = Name::new(format!("{}.bytes", self.prefix));
        let global = GlobalDeclaration {
            name: name.clone(),
            linkage: Linkage::Private,
            ty: Type::Array(bytes.len() as u64, Box::new(Type::Integer(8))),
            init: GlobalInit::Bytes(bytes),
            mutable: false,
            thread_local: false,
        };
        let address = self.emit(
            "source",
            Operation::GlobalAddress {
                global: name,
                thread_local: false,
            },
        );
        let source = self.emit(
            "source_pointer",
            Operation::Convert {
                kind: ConvertKind::Bitcast,
                value: address,
                ty: Type::Pointer,
            },
        );
        self.effect(Operation::MemCopy {
            dest: pointer.clone(),
            source,
            length: count,
        });
        (pointer, Some(global))
    }

    /// The value of field `index`, of type `ty`, which held `word`.
    fn field_value(
        &mut self,
        module: &Module,
        machine: &Machine<'_>,
        index: usize,
        ty: &Type,
        word: u64,
    ) -> Result<Name, String> {
        let suffix = format!("field_{index}");
        match ty {
            Type::Integer(_) => Ok(self.emit(
                &suffix,
                Operation::ConstInt {
                    ty: ty.clone(),
                    value: i128::from(word.cast_signed()),
                },
            )),
            Type::Pointer if word == 0 => Ok(self.emit(&suffix, Operation::ConstNull)),
            Type::Pointer => {
                let global = machine
                    .evaluation_global(word)
                    .ok_or("compile-time pointer cannot escape its private heap")?;
                let address = self.emit(
                    &format!("global_{index}"),
                    Operation::GlobalAddress {
                        global: module.globals[global].name.clone(),
                        thread_local: false,
                    },
                );
                Ok(self.emit(
                    &suffix,
                    Operation::Convert {
                        kind: ConvertKind::Bitcast,
                        value: address,
                        ty: Type::Pointer,
                    },
                ))
            }
            _ => Err("unsupported compile-time record field".to_owned()),
        }
    }

    /// Stores `value` into field `index` of the record at `destination`.
    fn store_field(&mut self, destination: &Name, index: usize, value: Name) -> Result<(), String> {
        let address = self.emit(
            &format!("field_address_{index}"),
            Operation::FieldAddr {
                base: destination.clone(),
                field: u32::try_from(index)
                    .map_err(|_| "compile-time record has too many fields")?,
            },
        );
        self.effect(Operation::Store {
            mode: StoreMode::Init,
            value,
            address,
        });
        Ok(())
    }
}
