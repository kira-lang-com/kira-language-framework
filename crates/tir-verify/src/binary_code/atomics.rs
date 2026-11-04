use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget, BinaryType, BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    target: BinaryTarget,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .filter_map(|block| block.instructions.as_deref())
        .flatten()
        .filter(|instruction| (0x200..=0x206).contains(&instruction.opcode))
    {
        verify_instruction(
            instruction,
            value_types,
            types,
            target,
            function_name,
            &mut diagnostics,
        );
    }
    diagnostics
}

fn verify_instruction(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    target: BinaryTarget,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let opcode = instruction.opcode;
    let values = values(instruction);
    let gpu_target = matches!(target, BinaryTarget::Gpu | BinaryTarget::Both);
    let gpu_only = target == BinaryTarget::Gpu;
    let scope = (instruction.modifiers >> 26) & 0x7;
    if scope != 0 && !(scope == 1 || scope == 5 || gpu_only && (2..=4).contains(&scope)) {
        report(
            diagnostics,
            function_name,
            instruction,
            "atomic scope is not valid for the function target",
        );
    }

    if matches!(opcode, 0x205 | 0x206) {
        verify_wait_notify(
            instruction,
            &values,
            value_types,
            types,
            function_name,
            diagnostics,
        );
        return;
    }
    if opcode == 0x204 {
        return;
    }

    let (value_type, pointer_index) = match opcode {
        0x200 => (explicit_type(instruction), 0),
        0x201 => (value_type_at(values.first(), value_types), 1),
        0x202 | 0x203 => (value_type_at(values.get(1), value_types), 0),
        _ => return,
    };
    let Some(value_type) = value_type else {
        return;
    };
    let Some(value_kind) = type_at(types, value_type) else {
        return;
    };
    if !is_atomic_memory_type(value_kind) {
        report(
            diagnostics,
            function_name,
            instruction,
            "atomic value type is not permitted by the TIR memory model",
        );
    }
    if !crate::binary_code::types::aggregate_instructions::is_trivial_type(types, value_type) {
        report(
            diagnostics,
            function_name,
            instruction,
            "atomic value type is not trivially copyable",
        );
    }
    match crate::binary_constants::binary_type_layout(types, value_type) {
        Ok(layout) if matches!(layout.size, 1 | 2 | 4 | 8 | 16) => {}
        Ok(layout) => report(
            diagnostics,
            function_name,
            instruction,
            format!(
                "atomic value type has unsupported width {} bytes",
                layout.size
            ),
        ),
        Err(_) => report(
            diagnostics,
            function_name,
            instruction,
            "atomic value type has no fixed layout",
        ),
    }
    let pointer_id = if opcode == 0x201 {
        values.get(pointer_index).copied()
    } else {
        values.first().copied()
    };
    if let Some(pointer_id) = pointer_id
        && let Some(pointer_type) = value_type_at(Some(&pointer_id), value_types)
        && !verify_pointer_type(pointer_type, value_type, types)
    {
        report(
            diagnostics,
            function_name,
            instruction,
            "atomic pointer is not a pointer to the atomic value type",
        );
    }
    if gpu_target && !matches!(opcode, 0x204..=0x206) {
        let space = pointer_id
            .and_then(|id| value_type_at(Some(&id), value_types))
            .and_then(|ty| pointer_space(types, ty));
        if !matches!(space, Some(1 | 3)) {
            report(
                diagnostics,
                function_name,
                instruction,
                "GPU atomics require a device or threadgroup pointer",
            );
        }
    }
    if opcode == 0x202 {
        verify_rmw_type(instruction, value_kind, function_name, diagnostics);
    }
    if opcode == 0x203
        && values
            .get(2)
            .and_then(|value| value_type_at(Some(value), value_types))
            .is_some_and(|new_type| new_type != value_type)
    {
        report(
            diagnostics,
            function_name,
            instruction,
            "atomic.cmpxchg expected and new values must have the same type",
        );
    }
}

fn verify_wait_notify(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let pointer = values
        .first()
        .and_then(|value| value_type_at(Some(value), value_types));
    let expected = values
        .get(1)
        .and_then(|value| value_type_at(Some(value), value_types));
    let pointee = pointer.and_then(|pointer| address_pointee(types, pointer));
    let word = |ty: Option<u64>| {
        ty.and_then(|ty| type_at(types, ty))
            .is_some_and(|ty| matches!(ty, BinaryType::Integer(32 | 64)))
    };
    if pointer.is_some_and(|pointer| !is_pointer(types, pointer)) || pointer.is_none() {
        report(
            diagnostics,
            function_name,
            instruction,
            "atomic wait/notify operand 1 is not a pointer",
        );
    }
    if let Some(pointee) = pointee
        && !word(Some(pointee))
    {
        report(
            diagnostics,
            function_name,
            instruction,
            "atomic wait/notify pointer must address i32 or i64",
        );
    }
    if instruction.opcode == 0x205 {
        if !word(expected) || pointee.is_some_and(|pointee| Some(pointee) != expected) {
            report(
                diagnostics,
                function_name,
                instruction,
                "atomic.wait expected value must match its i32 or i64 pointer",
            );
        }
        if values
            .get(2)
            .and_then(|value| value_type_at(Some(value), value_types))
            .and_then(|ty| type_at(types, ty))
            .is_some_and(|ty| !matches!(ty, BinaryType::Integer(64)))
        {
            report(
                diagnostics,
                function_name,
                instruction,
                "atomic.wait timeout must have type i64",
            );
        }
    } else if expected
        .and_then(|ty| type_at(types, ty))
        .is_some_and(|ty| !matches!(ty, BinaryType::Integer(32)))
    {
        report(
            diagnostics,
            function_name,
            instruction,
            "atomic.notify count must have type i32",
        );
    }
}

fn verify_rmw_type(
    instruction: &BinaryInstruction,
    value_type: &BinaryType,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let operation = (instruction.modifiers >> 48) & 0xf;
    let integer = matches!(operation, 1..=10 | 15..=18);
    let float = matches!(operation, 11..=14);
    if integer && !matches!(value_type, BinaryType::Integer(_))
        || float && !matches!(value_type, BinaryType::Float(_))
    {
        report(
            diagnostics,
            function_name,
            instruction,
            "atomic.rmw operation is incompatible with its scalar value type",
        );
    }
}

fn is_atomic_memory_type(ty: &BinaryType) -> bool {
    !matches!(
        ty,
        BinaryType::Integer(1)
            | BinaryType::Unit
            | BinaryType::Never
            | BinaryType::ScalableVector { .. }
            | BinaryType::Reference { .. }
            | BinaryType::Unique { .. }
            | BinaryType::Weak { .. }
            | BinaryType::Address { .. }
            | BinaryType::Function(_)
            | BinaryType::Closure { .. }
            | BinaryType::Coroutine { .. }
            | BinaryType::Frame { .. }
            | BinaryType::RayQuery
            | BinaryType::HitObject
            | BinaryType::CooperativeMatrix { .. }
            | BinaryType::Entry { .. }
            | BinaryType::Generic { .. }
    )
}

fn verify_pointer_type(pointer: u64, value_type: u64, types: &BinaryTypeTable) -> bool {
    match type_at(types, pointer) {
        Some(BinaryType::Pointer(_)) => true,
        Some(BinaryType::Address { pointee, .. }) => *pointee == value_type,
        _ => false,
    }
}

fn pointer_space(types: &BinaryTypeTable, pointer: u64) -> Option<u8> {
    match type_at(types, pointer)? {
        BinaryType::Pointer(space) | BinaryType::Address { space, .. } => Some(*space),
        _ => None,
    }
}

fn is_pointer(types: &BinaryTypeTable, ty: u64) -> bool {
    matches!(
        type_at(types, ty),
        Some(BinaryType::Pointer(_) | BinaryType::Address { .. })
    )
}

fn address_pointee(types: &BinaryTypeTable, ty: u64) -> Option<u64> {
    match type_at(types, ty)? {
        BinaryType::Address { pointee, .. } => Some(*pointee),
        _ => None,
    }
}

fn explicit_type(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(ty) => Some(*ty),
            _ => None,
        })
}

fn values(instruction: &BinaryInstruction) -> Vec<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Values(values) => Some(values.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn value_type_at(value: Option<&u64>, value_types: &[Option<u64>]) -> Option<u64> {
    value
        .and_then(|value| usize::try_from(*value).ok())
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn type_at(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}

fn report(
    diagnostics: &mut Vec<Diagnostic>,
    function_name: &str,
    instruction: &BinaryInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(
        V2,
        format!(
            "function @{function_name} CODE offset {}: {}",
            instruction.offset,
            message.into()
        ),
    ));
}
