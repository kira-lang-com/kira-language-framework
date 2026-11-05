use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind};
use tir_types::{AddressSpace, FloatType, Type};

use crate::ssa::text_instruction_operands_start;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(instruction: &TextInstruction, operands: &[Type]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    match instruction.opcode.as_str() {
        "rq.init" => verify_ray_query_init(instruction, operands, &mut diagnostics),
        "rq.proceed" | "rq.commit_triangle" | "rq.abort" | "rq.get" => require_pointer_space(
            instruction,
            operands,
            0,
            AddressSpace::Thread,
            &mut diagnostics,
        ),
        "rq.commit_procedural" => {
            require_pointer_space(
                instruction,
                operands,
                0,
                AddressSpace::Thread,
                &mut diagnostics,
            );
            if operands.get(1).is_some_and(|ty| ty != &f32()) {
                error(
                    &mut diagnostics,
                    instruction,
                    "rq.commit_procedural t must have type f32",
                );
            }
        }
        "rt.trace" => verify_ray_trace(instruction, operands, &mut diagnostics),
        "rt.report_hit" => verify_report_hit(instruction, operands, &mut diagnostics),
        "rt.call_callable" => {
            if operands.first().is_some_and(|ty| !is_integer_type(ty)) {
                error(
                    &mut diagnostics,
                    instruction,
                    "rt.call_callable index must be an integer",
                );
            }
            if operands
                .get(1)
                .is_some_and(|ty| !matches!(ty, Type::Pointer(_)))
            {
                error(
                    &mut diagnostics,
                    instruction,
                    "rt.call_callable data must be a pointer",
                );
            }
        }
        "rt.hitobject.from_query" => require_pointer_space(
            instruction,
            operands,
            0,
            AddressSpace::Thread,
            &mut diagnostics,
        ),
        "rt.hitobject.invoke" => {
            if operands.first().is_some_and(|ty| ty != &Type::HitObject) {
                error(
                    &mut diagnostics,
                    instruction,
                    "rt.hitobject.invoke requires a hitobject",
                );
            }
            require_pointer_space(
                instruction,
                operands,
                1,
                AddressSpace::RayPayload,
                &mut diagnostics,
            );
        }
        "rt.hitobject.get" | "rt.reorder" => {
            if operands.first().is_some_and(|ty| ty != &Type::HitObject) {
                error(
                    &mut diagnostics,
                    instruction,
                    format!("{} requires a hitobject value", instruction.opcode),
                );
            }
            if instruction.opcode == "rt.reorder"
                && operands.get(1).is_some_and(|ty| !is_integer_type(ty))
            {
                error(
                    &mut diagnostics,
                    instruction,
                    "rt.reorder hint must be an integer",
                );
            }
        }
        _ => {}
    }
    diagnostics
}

pub(super) fn infer_result_types(instruction: &TextInstruction) -> Vec<Type> {
    match instruction.opcode.as_str() {
        "rq.proceed" | "rt.report_hit" => vec![Type::Integer(1)],
        "rq.get" | "rt.get" | "rt.hitobject.get" => ray_attribute_types(instruction),
        "rt.hitobject.trace" | "rt.hitobject.from_query" | "rt.hitobject.miss" => {
            vec![Type::HitObject]
        }
        _ => Vec::new(),
    }
}

fn verify_ray_query_init(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    require_pointer_space(instruction, operands, 0, AddressSpace::Thread, diagnostics);
    if operands
        .get(1)
        .is_some_and(|ty| ty != &Type::AccelerationStructure)
    {
        error(
            diagnostics,
            instruction,
            "rq.init requires an acceleration structure",
        );
    }
    for index in [2, 3] {
        if operands.get(index).is_some_and(|ty| !is_integer_type(ty)) {
            error(
                diagnostics,
                instruction,
                "rq.init flags and mask must be integers",
            );
            break;
        }
    }
    for index in [4, 6] {
        if operands
            .get(index)
            .is_some_and(|ty| ty != &vector(3, f32()))
        {
            error(
                diagnostics,
                instruction,
                "rq.init origin and direction must have type vec<3 x f32>",
            );
            break;
        }
    }
    for index in [5, 7] {
        if operands.get(index).is_some_and(|ty| ty != &f32()) {
            error(
                diagnostics,
                instruction,
                "rq.init tmin and tmax must have type f32",
            );
            break;
        }
    }
}

fn verify_ray_trace(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands
        .first()
        .is_some_and(|ty| ty != &Type::AccelerationStructure)
    {
        error(
            diagnostics,
            instruction,
            "rt.trace requires an acceleration structure",
        );
    }
    if operands
        .iter()
        .take(6)
        .skip(1)
        .any(|ty| !is_integer_type(ty))
    {
        error(
            diagnostics,
            instruction,
            "rt.trace flags, mask, SBT values, and miss index must be integers",
        );
    }
    for index in [6, 8] {
        if operands
            .get(index)
            .is_some_and(|ty| ty != &vector(3, f32()))
        {
            error(
                diagnostics,
                instruction,
                "rt.trace origin and direction must have type vec<3 x f32>",
            );
            break;
        }
    }
    for index in [7, 9] {
        if operands.get(index).is_some_and(|ty| ty != &f32()) {
            error(
                diagnostics,
                instruction,
                "rt.trace tmin and tmax must have type f32",
            );
            break;
        }
    }
    require_pointer_space(
        instruction,
        operands,
        10,
        AddressSpace::RayPayload,
        diagnostics,
    );
}

fn verify_report_hit(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands.first().is_some_and(|ty| ty != &f32()) {
        error(
            diagnostics,
            instruction,
            "rt.report_hit t must have type f32",
        );
    }
    if operands.get(1).is_some_and(|ty| !is_integer_type(ty)) {
        error(
            diagnostics,
            instruction,
            "rt.report_hit kind must be an integer",
        );
    }
    require_pointer_space(
        instruction,
        operands,
        2,
        AddressSpace::HitAttribute,
        diagnostics,
    );
}

fn require_pointer_space(
    instruction: &TextInstruction,
    operands: &[Type],
    index: usize,
    space: AddressSpace,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands
        .get(index)
        .is_some_and(|ty| ty != &Type::Pointer(space))
    {
        error(
            diagnostics,
            instruction,
            format!(
                "{} operand {} must have ptr<{}> type",
                instruction.opcode,
                index + 1,
                space.name()
            ),
        );
    }
}

fn ray_attribute_types(instruction: &TextInstruction) -> Vec<Type> {
    let Some(attribute) = instruction_attribute(instruction) else {
        return Vec::new();
    };
    if matches!(instruction.opcode.as_str(), "rq.get" | "rt.get")
        && matches!(attribute, "object_to_world" | "world_to_object")
    {
        return vec![f32(); 12];
    }
    if instruction.opcode == "rt.get" && attribute == "triangle_vertex_positions" {
        return vec![f32(); 9];
    }
    let ty = match attribute {
        "kind"
        | "instance_id"
        | "instance_custom_index"
        | "geometry_index"
        | "primitive_index"
        | "ray_flags"
        | "hit_kind" => Type::Integer(32),
        "t" | "ray_tmin" | "ray_tcurrent" => f32(),
        "front_face" => Type::Integer(1),
        "barycentrics" => vector(2, f32()),
        "object_ray_origin"
        | "object_ray_direction"
        | "world_ray_origin"
        | "world_ray_direction" => vector(3, f32()),
        "launch_id" | "launch_size" => vector(3, Type::Integer(32)),
        _ => return Vec::new(),
    };
    vec![ty]
}

fn instruction_attribute(instruction: &TextInstruction) -> Option<&str> {
    let start = text_instruction_operands_start(&instruction.tokens);
    if instruction.opcode == "rt.get" {
        return instruction.tokens.get(start).and_then(atom);
    }
    let comma = instruction.tokens[start..]
        .iter()
        .position(|token| is_punctuation(token, ','))?
        + start;
    instruction.tokens.get(comma + 1).and_then(atom)
}

fn is_integer_type(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_))
}

fn f32() -> Type {
    Type::Float(FloatType::F32)
}

fn vector(lanes: u32, element: Type) -> Type {
    Type::FixedVector {
        lanes,
        element: Box::new(element),
    }
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn error(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(V2, message.into()).with_primary_span(instruction.span));
}
