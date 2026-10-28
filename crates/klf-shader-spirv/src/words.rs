//! SPIR-V words: instruction encoding and the numeric values of the
//! opcodes and enumerants the backend uses.

/// One instruction: the opcode and its operand words.
pub(crate) fn instruction(opcode: u16, operands: &[u32]) -> Vec<u32> {
    let mut words = Vec::with_capacity(operands.len() + 1);
    let count = u32::try_from(operands.len() + 1).unwrap_or(u32::MAX);
    words.push((count << 16) | u32::from(opcode));
    words.extend_from_slice(operands);
    words
}

/// A literal string: UTF-8, nul-terminated, padded to a word.
pub(crate) fn string(text: &str) -> Vec<u32> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    bytes
        .chunks(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

pub(crate) mod op {
    pub const NAME: u16 = 5;
    pub const MEMBER_NAME: u16 = 6;
    pub const EXT_INST_IMPORT: u16 = 11;
    pub const EXT_INST: u16 = 12;
    pub const MEMORY_MODEL: u16 = 14;
    pub const ENTRY_POINT: u16 = 15;
    pub const EXECUTION_MODE: u16 = 16;
    pub const CAPABILITY: u16 = 17;
    pub const TYPE_VOID: u16 = 19;
    pub const TYPE_BOOL: u16 = 20;
    pub const TYPE_INT: u16 = 21;
    pub const TYPE_FLOAT: u16 = 22;
    pub const TYPE_VECTOR: u16 = 23;
    pub const TYPE_MATRIX: u16 = 24;
    pub const TYPE_IMAGE: u16 = 25;
    pub const TYPE_SAMPLER: u16 = 26;
    pub const TYPE_SAMPLED_IMAGE: u16 = 27;
    pub const TYPE_ARRAY: u16 = 28;
    pub const TYPE_RUNTIME_ARRAY: u16 = 29;
    pub const TYPE_STRUCT: u16 = 30;
    pub const TYPE_POINTER: u16 = 32;
    pub const TYPE_FUNCTION: u16 = 33;
    pub const SOURCE: u16 = 3;
    pub const STRING: u16 = 7;
    pub const LINE: u16 = 8;
    pub const CONSTANT_TRUE: u16 = 41;
    pub const CONSTANT_FALSE: u16 = 42;
    pub const CONSTANT: u16 = 43;
    pub const CONSTANT_COMPOSITE: u16 = 44;
    pub const CONSTANT_NULL: u16 = 46;
    pub const SPEC_CONSTANT_TRUE: u16 = 48;
    pub const SPEC_CONSTANT_FALSE: u16 = 49;
    pub const SPEC_CONSTANT: u16 = 50;
    pub const SPEC_CONSTANT_COMPOSITE: u16 = 51;
    pub const FUNCTION: u16 = 54;
    pub const FUNCTION_PARAMETER: u16 = 55;
    pub const FUNCTION_END: u16 = 56;
    pub const FUNCTION_CALL: u16 = 57;
    pub const VARIABLE: u16 = 59;
    pub const LOAD: u16 = 61;
    pub const STORE: u16 = 62;
    pub const ACCESS_CHAIN: u16 = 65;
    pub const ARRAY_LENGTH: u16 = 68;
    pub const DECORATE: u16 = 71;
    pub const MEMBER_DECORATE: u16 = 72;
    pub const VECTOR_EXTRACT_DYNAMIC: u16 = 77;
    pub const VECTOR_SHUFFLE: u16 = 79;
    pub const COMPOSITE_CONSTRUCT: u16 = 80;
    pub const COMPOSITE_EXTRACT: u16 = 81;
    pub const TRANSPOSE: u16 = 84;
    pub const SAMPLED_IMAGE: u16 = 86;
    pub const IMAGE_SAMPLE_IMPLICIT_LOD: u16 = 87;
    pub const IMAGE_SAMPLE_EXPLICIT_LOD: u16 = 88;
    pub const IMAGE_SAMPLE_DREF_IMPLICIT_LOD: u16 = 89;
    pub const IMAGE_SAMPLE_DREF_EXPLICIT_LOD: u16 = 90;
    pub const IMAGE_FETCH: u16 = 95;
    pub const IMAGE_GATHER: u16 = 96;
    pub const IMAGE_DREF_GATHER: u16 = 97;
    pub const IMAGE_READ: u16 = 98;
    pub const IMAGE_WRITE: u16 = 99;
    pub const IMAGE_QUERY_SIZE_LOD: u16 = 103;
    pub const IMAGE_QUERY_SIZE: u16 = 104;
    pub const IMAGE_QUERY_LEVELS: u16 = 106;
    pub const IMAGE_QUERY_SAMPLES: u16 = 107;
    pub const CONVERT_F_TO_U: u16 = 109;
    pub const CONVERT_F_TO_S: u16 = 110;
    pub const CONVERT_S_TO_F: u16 = 111;
    pub const CONVERT_U_TO_F: u16 = 112;
    pub const U_CONVERT: u16 = 113;
    pub const S_CONVERT: u16 = 114;
    pub const F_CONVERT: u16 = 115;
    pub const BITCAST: u16 = 124;
    pub const S_NEGATE: u16 = 126;
    pub const F_NEGATE: u16 = 127;
    pub const I_ADD: u16 = 128;
    pub const F_ADD: u16 = 129;
    pub const I_SUB: u16 = 130;
    pub const F_SUB: u16 = 131;
    pub const I_MUL: u16 = 132;
    pub const F_MUL: u16 = 133;
    pub const U_DIV: u16 = 134;
    pub const S_DIV: u16 = 135;
    pub const F_DIV: u16 = 136;
    pub const U_MOD: u16 = 137;
    pub const S_REM: u16 = 138;
    pub const F_REM: u16 = 140;
    pub const VECTOR_TIMES_SCALAR: u16 = 142;
    pub const MATRIX_TIMES_SCALAR: u16 = 143;
    pub const VECTOR_TIMES_MATRIX: u16 = 144;
    pub const MATRIX_TIMES_VECTOR: u16 = 145;
    pub const MATRIX_TIMES_MATRIX: u16 = 146;
    pub const DOT: u16 = 148;
    pub const ANY: u16 = 154;
    pub const ALL: u16 = 155;
    pub const IS_NAN: u16 = 156;
    pub const IS_INF: u16 = 157;
    pub const LOGICAL_EQUAL: u16 = 164;
    pub const LOGICAL_NOT_EQUAL: u16 = 165;
    pub const LOGICAL_OR: u16 = 166;
    pub const LOGICAL_AND: u16 = 167;
    pub const LOGICAL_NOT: u16 = 168;
    pub const SELECT: u16 = 169;
    pub const I_EQUAL: u16 = 170;
    pub const I_NOT_EQUAL: u16 = 171;
    pub const U_GREATER_THAN: u16 = 172;
    pub const S_GREATER_THAN: u16 = 173;
    pub const U_GREATER_THAN_EQUAL: u16 = 174;
    pub const S_GREATER_THAN_EQUAL: u16 = 175;
    pub const U_LESS_THAN: u16 = 176;
    pub const S_LESS_THAN: u16 = 177;
    pub const U_LESS_THAN_EQUAL: u16 = 178;
    pub const S_LESS_THAN_EQUAL: u16 = 179;
    pub const F_ORD_EQUAL: u16 = 180;
    pub const F_UNORD_NOT_EQUAL: u16 = 183;
    pub const F_ORD_LESS_THAN: u16 = 184;
    pub const F_ORD_GREATER_THAN: u16 = 186;
    pub const F_ORD_LESS_THAN_EQUAL: u16 = 188;
    pub const F_ORD_GREATER_THAN_EQUAL: u16 = 190;
    pub const SHIFT_RIGHT_LOGICAL: u16 = 194;
    pub const SHIFT_RIGHT_ARITHMETIC: u16 = 195;
    pub const SHIFT_LEFT_LOGICAL: u16 = 196;
    pub const BITWISE_OR: u16 = 197;
    pub const BITWISE_XOR: u16 = 198;
    pub const BITWISE_AND: u16 = 199;
    pub const NOT: u16 = 200;
    pub const BIT_FIELD_INSERT: u16 = 201;
    pub const BIT_FIELD_S_EXTRACT: u16 = 202;
    pub const BIT_FIELD_U_EXTRACT: u16 = 203;
    pub const BIT_REVERSE: u16 = 204;
    pub const BIT_COUNT: u16 = 205;
    pub const DPDX: u16 = 207;
    pub const DPDY: u16 = 208;
    pub const FWIDTH: u16 = 209;
    pub const DPDX_FINE: u16 = 210;
    pub const DPDY_FINE: u16 = 211;
    pub const FWIDTH_FINE: u16 = 212;
    pub const DPDX_COARSE: u16 = 213;
    pub const DPDY_COARSE: u16 = 214;
    pub const FWIDTH_COARSE: u16 = 215;
    pub const CONTROL_BARRIER: u16 = 224;
    pub const MEMORY_BARRIER: u16 = 225;
    pub const ATOMIC_LOAD: u16 = 227;
    pub const ATOMIC_STORE: u16 = 228;
    pub const ATOMIC_EXCHANGE: u16 = 229;
    pub const ATOMIC_COMPARE_EXCHANGE: u16 = 230;
    pub const ATOMIC_I_ADD: u16 = 234;
    pub const ATOMIC_I_SUB: u16 = 235;
    pub const ATOMIC_S_MIN: u16 = 236;
    pub const ATOMIC_U_MIN: u16 = 237;
    pub const ATOMIC_S_MAX: u16 = 238;
    pub const ATOMIC_U_MAX: u16 = 239;
    pub const ATOMIC_AND: u16 = 240;
    pub const ATOMIC_OR: u16 = 241;
    pub const ATOMIC_XOR: u16 = 242;
    pub const LOOP_MERGE: u16 = 246;
    pub const SELECTION_MERGE: u16 = 247;
    pub const LABEL: u16 = 248;
    pub const BRANCH: u16 = 249;
    pub const BRANCH_CONDITIONAL: u16 = 250;
    pub const SWITCH: u16 = 251;
    pub const KILL: u16 = 252;
    pub const UNREACHABLE: u16 = 255;
    pub const RETURN: u16 = 253;
    pub const RETURN_VALUE: u16 = 254;
}

pub(crate) mod decoration {
    pub const SPEC_ID: u32 = 1;
    pub const BLOCK: u32 = 2;
    pub const COL_MAJOR: u32 = 5;
    pub const ARRAY_STRIDE: u32 = 6;
    pub const MATRIX_STRIDE: u32 = 7;
    pub const BUILT_IN: u32 = 11;
    pub const NO_PERSPECTIVE: u32 = 13;
    pub const FLAT: u32 = 14;
    pub const CENTROID: u32 = 16;
    pub const SAMPLE: u32 = 17;
    pub const NON_WRITABLE: u32 = 24;
    pub const NON_READABLE: u32 = 25;
    pub const LOCATION: u32 = 30;
    pub const INDEX: u32 = 32;
    pub const BINDING: u32 = 33;
    pub const DESCRIPTOR_SET: u32 = 34;
    pub const OFFSET: u32 = 35;
}

pub(crate) mod class {
    pub const UNIFORM_CONSTANT: u32 = 0;
    pub const INPUT: u32 = 1;
    pub const UNIFORM: u32 = 2;
    pub const OUTPUT: u32 = 3;
    pub const WORKGROUP: u32 = 4;
    pub const PRIVATE: u32 = 6;
    pub const FUNCTION: u32 = 7;
    pub const PUSH_CONSTANT: u32 = 9;
    pub const STORAGE_BUFFER: u32 = 12;
}

pub(crate) mod capability {
    pub const SHADER: u32 = 1;
    pub const FLOAT16: u32 = 9;
    pub const FLOAT64: u32 = 10;
    pub const INT64: u32 = 11;
    pub const INT64_ATOMICS: u32 = 12;
    pub const IMAGE_GATHER_EXTENDED: u32 = 25;
    pub const CLIP_DISTANCE: u32 = 32;
    pub const STORAGE_IMAGE_MULTISAMPLE: u32 = 27;
    pub const IMAGE_CUBE_ARRAY: u32 = 34;
    pub const SAMPLE_RATE_SHADING: u32 = 35;
    pub const SAMPLED_1D: u32 = 43;
    pub const IMAGE_1D: u32 = 44;
    pub const SAMPLED_CUBE_ARRAY: u32 = 45;
    pub const STORAGE_IMAGE_EXTENDED_FORMATS: u32 = 49;
    pub const IMAGE_QUERY: u32 = 50;
    pub const DERIVATIVE_CONTROL: u32 = 51;
}

/// GLSL.std.450 extended instructions.
pub(crate) mod glsl {
    pub const ROUND_EVEN: u32 = 2;
    pub const TRUNC: u32 = 3;
    pub const F_ABS: u32 = 4;
    pub const S_ABS: u32 = 5;
    pub const F_SIGN: u32 = 6;
    pub const S_SIGN: u32 = 7;
    pub const FLOOR: u32 = 8;
    pub const CEIL: u32 = 9;
    pub const FRACT: u32 = 10;
    pub const RADIANS: u32 = 11;
    pub const DEGREES: u32 = 12;
    pub const SIN: u32 = 13;
    pub const COS: u32 = 14;
    pub const TAN: u32 = 15;
    pub const ASIN: u32 = 16;
    pub const ACOS: u32 = 17;
    pub const ATAN: u32 = 18;
    pub const SINH: u32 = 19;
    pub const COSH: u32 = 20;
    pub const TANH: u32 = 21;
    pub const ASINH: u32 = 22;
    pub const ACOSH: u32 = 23;
    pub const ATANH: u32 = 24;
    pub const ATAN2: u32 = 25;
    pub const POW: u32 = 26;
    pub const EXP: u32 = 27;
    pub const LOG: u32 = 28;
    pub const EXP2: u32 = 29;
    pub const LOG2: u32 = 30;
    pub const SQRT: u32 = 31;
    pub const INVERSE_SQRT: u32 = 32;
    pub const DETERMINANT: u32 = 33;
    pub const F_MIN: u32 = 37;
    pub const U_MIN: u32 = 38;
    pub const S_MIN: u32 = 39;
    pub const F_MAX: u32 = 40;
    pub const U_MAX: u32 = 41;
    pub const S_MAX: u32 = 42;
    pub const F_CLAMP: u32 = 43;
    pub const U_CLAMP: u32 = 44;
    pub const S_CLAMP: u32 = 45;
    pub const F_MIX: u32 = 46;
    pub const STEP: u32 = 48;
    pub const SMOOTH_STEP: u32 = 49;
    pub const FMA: u32 = 50;
    pub const PACK_SNORM_4X8: u32 = 54;
    pub const PACK_UNORM_4X8: u32 = 55;
    pub const PACK_SNORM_2X16: u32 = 56;
    pub const PACK_UNORM_2X16: u32 = 57;
    pub const PACK_HALF_2X16: u32 = 58;
    pub const UNPACK_SNORM_2X16: u32 = 60;
    pub const UNPACK_UNORM_2X16: u32 = 61;
    pub const UNPACK_HALF_2X16: u32 = 62;
    pub const UNPACK_SNORM_4X8: u32 = 63;
    pub const UNPACK_UNORM_4X8: u32 = 64;
    pub const LENGTH: u32 = 66;
    pub const DISTANCE: u32 = 67;
    pub const CROSS: u32 = 68;
    pub const NORMALIZE: u32 = 69;
    pub const FACE_FORWARD: u32 = 70;
    pub const REFLECT: u32 = 71;
    pub const REFRACT: u32 = 72;
    pub const FIND_I_LSB: u32 = 73;
    pub const FIND_S_MSB: u32 = 74;
    pub const FIND_U_MSB: u32 = 75;
}
