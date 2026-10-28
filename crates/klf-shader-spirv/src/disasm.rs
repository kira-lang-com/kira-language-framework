//! Disassembly of the SPIR-V this crate writes (`KLF-EDITOR.md` section
//! 14.5), so an editor can show a shader's SPIR-V without a tool on the
//! machine.
//!
//! The stream is decoded as SPIR-V lays it out: a word count, an opcode and
//! that many operand words. An opcode the backend emits is named from its
//! own table; anything else is `Op<number>`, which keeps a view readable for
//! a module this writer did not produce.

use crate::words::op;
use std::fmt::Write;

/// The first word of every SPIR-V module.
const MAGIC: u32 = 0x0723_0203;

#[rustfmt::skip]
const OPCODES: [(u16, &str); 164] = [
    (3, "OpSource"),
    (5, "OpName"),
    (6, "OpMemberName"),
    (7, "OpString"),
    (8, "OpLine"),
    (11, "OpExtInstImport"),
    (12, "OpExtInst"),
    (14, "OpMemoryModel"),
    (15, "OpEntryPoint"),
    (16, "OpExecutionMode"),
    (17, "OpCapability"),
    (19, "OpTypeVoid"),
    (20, "OpTypeBool"),
    (21, "OpTypeInt"),
    (22, "OpTypeFloat"),
    (23, "OpTypeVector"),
    (24, "OpTypeMatrix"),
    (25, "OpTypeImage"),
    (26, "OpTypeSampler"),
    (27, "OpTypeSampledImage"),
    (28, "OpTypeArray"),
    (29, "OpTypeRuntimeArray"),
    (30, "OpTypeStruct"),
    (32, "OpTypePointer"),
    (33, "OpTypeFunction"),
    (41, "OpConstantTrue"),
    (42, "OpConstantFalse"),
    (43, "OpConstant"),
    (44, "OpConstantComposite"),
    (46, "OpConstantNull"),
    (48, "OpSpecConstantTrue"),
    (49, "OpSpecConstantFalse"),
    (50, "OpSpecConstant"),
    (51, "OpSpecConstantComposite"),
    (54, "OpFunction"),
    (55, "OpFunctionParameter"),
    (56, "OpFunctionEnd"),
    (57, "OpFunctionCall"),
    (59, "OpVariable"),
    (61, "OpLoad"),
    (62, "OpStore"),
    (65, "OpAccessChain"),
    (68, "OpArrayLength"),
    (71, "OpDecorate"),
    (72, "OpMemberDecorate"),
    (77, "OpVectorExtractDynamic"),
    (79, "OpVectorShuffle"),
    (80, "OpCompositeConstruct"),
    (81, "OpCompositeExtract"),
    (84, "OpTranspose"),
    (86, "OpSampledImage"),
    (87, "OpImageSampleImplicitLod"),
    (88, "OpImageSampleExplicitLod"),
    (89, "OpImageSampleDrefImplicitLod"),
    (90, "OpImageSampleDrefExplicitLod"),
    (95, "OpImageFetch"),
    (96, "OpImageGather"),
    (97, "OpImageDrefGather"),
    (98, "OpImageRead"),
    (99, "OpImageWrite"),
    (103, "OpImageQuerySizeLod"),
    (104, "OpImageQuerySize"),
    (106, "OpImageQueryLevels"),
    (107, "OpImageQuerySamples"),
    (109, "OpConvertFToU"),
    (110, "OpConvertFToS"),
    (111, "OpConvertSToF"),
    (112, "OpConvertUToF"),
    (113, "OpUConvert"),
    (114, "OpSConvert"),
    (115, "OpFConvert"),
    (124, "OpBitcast"),
    (126, "OpSNegate"),
    (127, "OpFNegate"),
    (128, "OpIAdd"),
    (129, "OpFAdd"),
    (130, "OpISub"),
    (131, "OpFSub"),
    (132, "OpIMul"),
    (133, "OpFMul"),
    (134, "OpUDiv"),
    (135, "OpSDiv"),
    (136, "OpFDiv"),
    (137, "OpUMod"),
    (138, "OpSRem"),
    (140, "OpFRem"),
    (142, "OpVectorTimesScalar"),
    (143, "OpMatrixTimesScalar"),
    (144, "OpVectorTimesMatrix"),
    (145, "OpMatrixTimesVector"),
    (146, "OpMatrixTimesMatrix"),
    (148, "OpDot"),
    (154, "OpAny"),
    (155, "OpAll"),
    (156, "OpIsNan"),
    (157, "OpIsInf"),
    (164, "OpLogicalEqual"),
    (165, "OpLogicalNotEqual"),
    (166, "OpLogicalOr"),
    (167, "OpLogicalAnd"),
    (168, "OpLogicalNot"),
    (169, "OpSelect"),
    (170, "OpIEqual"),
    (171, "OpINotEqual"),
    (172, "OpUGreaterThan"),
    (173, "OpSGreaterThan"),
    (174, "OpUGreaterThanEqual"),
    (175, "OpSGreaterThanEqual"),
    (176, "OpULessThan"),
    (177, "OpSLessThan"),
    (178, "OpULessThanEqual"),
    (179, "OpSLessThanEqual"),
    (180, "OpFOrdEqual"),
    (183, "OpFUnordNotEqual"),
    (184, "OpFOrdLessThan"),
    (186, "OpFOrdGreaterThan"),
    (188, "OpFOrdLessThanEqual"),
    (190, "OpFOrdGreaterThanEqual"),
    (194, "OpShiftRightLogical"),
    (195, "OpShiftRightArithmetic"),
    (196, "OpShiftLeftLogical"),
    (197, "OpBitwiseOr"),
    (198, "OpBitwiseXor"),
    (199, "OpBitwiseAnd"),
    (200, "OpNot"),
    (201, "OpBitFieldInsert"),
    (202, "OpBitFieldSExtract"),
    (203, "OpBitFieldUExtract"),
    (204, "OpBitReverse"),
    (205, "OpBitCount"),
    (207, "OpDpdx"),
    (208, "OpDpdy"),
    (209, "OpFwidth"),
    (210, "OpDpdxFine"),
    (211, "OpDpdyFine"),
    (212, "OpFwidthFine"),
    (213, "OpDpdxCoarse"),
    (214, "OpDpdyCoarse"),
    (215, "OpFwidthCoarse"),
    (224, "OpControlBarrier"),
    (225, "OpMemoryBarrier"),
    (227, "OpAtomicLoad"),
    (228, "OpAtomicStore"),
    (229, "OpAtomicExchange"),
    (230, "OpAtomicCompareExchange"),
    (234, "OpAtomicIAdd"),
    (235, "OpAtomicISub"),
    (236, "OpAtomicSMin"),
    (237, "OpAtomicUMin"),
    (238, "OpAtomicSMax"),
    (239, "OpAtomicUMax"),
    (240, "OpAtomicAnd"),
    (241, "OpAtomicOr"),
    (242, "OpAtomicXor"),
    (246, "OpLoopMerge"),
    (247, "OpSelectionMerge"),
    (248, "OpLabel"),
    (249, "OpBranch"),
    (250, "OpBranchConditional"),
    (251, "OpSwitch"),
    (252, "OpKill"),
    (253, "OpReturn"),
    (254, "OpReturnValue"),
    (255, "OpUnreachable"),
];

/// An opcode that leaves a result, and whether it leaves a result type before
/// it. Every opcode the writer emits is here, so a `%id =` is never guessed.
const RESULTS: &[(u16, bool)] = &[
    (op::EXT_INST, true),
    (op::FUNCTION, true),
    (op::VARIABLE, true),
    (op::LOAD, true),
    (op::STORE, false),
    (op::ACCESS_CHAIN, true),
    (op::ARRAY_LENGTH, true),
    (op::VECTOR_EXTRACT_DYNAMIC, true),
    (op::VECTOR_SHUFFLE, true),
    (op::COMPOSITE_CONSTRUCT, true),
    (op::COMPOSITE_EXTRACT, true),
    (op::TRANSPOSE, true),
    (op::SAMPLED_IMAGE, true),
    (op::IMAGE_SAMPLE_IMPLICIT_LOD, true),
    (op::IMAGE_SAMPLE_EXPLICIT_LOD, true),
    (op::IMAGE_SAMPLE_DREF_IMPLICIT_LOD, true),
    (op::IMAGE_SAMPLE_DREF_EXPLICIT_LOD, true),
    (op::IMAGE_FETCH, true),
    (op::IMAGE_GATHER, true),
    (op::IMAGE_DREF_GATHER, true),
    (op::IMAGE_READ, true),
    (op::IMAGE_WRITE, false),
    (op::IMAGE_QUERY_SIZE_LOD, true),
    (op::IMAGE_QUERY_SIZE, true),
    (op::IMAGE_QUERY_LEVELS, true),
    (op::IMAGE_QUERY_SAMPLES, true),
    (op::CONVERT_F_TO_U, true),
    (op::CONVERT_F_TO_S, true),
    (op::CONVERT_S_TO_F, true),
    (op::CONVERT_U_TO_F, true),
    (op::U_CONVERT, true),
    (op::S_CONVERT, true),
    (op::F_CONVERT, true),
    (op::BITCAST, true),
    (op::S_NEGATE, true),
    (op::F_NEGATE, true),
    (op::I_ADD, true),
    (op::F_ADD, true),
    (op::I_SUB, true),
    (op::F_SUB, true),
    (op::I_MUL, true),
    (op::F_MUL, true),
    (op::U_DIV, true),
    (op::S_DIV, true),
    (op::F_DIV, true),
    (op::U_MOD, true),
    (op::S_REM, true),
    (op::F_REM, true),
    (op::VECTOR_TIMES_SCALAR, true),
    (op::MATRIX_TIMES_SCALAR, true),
    (op::VECTOR_TIMES_MATRIX, true),
    (op::MATRIX_TIMES_VECTOR, true),
    (op::MATRIX_TIMES_MATRIX, true),
    (op::DOT, true),
    (op::ANY, true),
    (op::ALL, true),
    (op::IS_NAN, true),
    (op::IS_INF, true),
    (op::LOGICAL_EQUAL, true),
    (op::LOGICAL_NOT_EQUAL, true),
    (op::LOGICAL_OR, true),
    (op::LOGICAL_AND, true),
    (op::LOGICAL_NOT, true),
    (op::SELECT, true),
    (op::I_EQUAL, true),
    (op::I_NOT_EQUAL, true),
    (op::U_GREATER_THAN, true),
    (op::S_GREATER_THAN, true),
    (op::U_GREATER_THAN_EQUAL, true),
    (op::S_GREATER_THAN_EQUAL, true),
    (op::U_LESS_THAN, true),
    (op::S_LESS_THAN, true),
    (op::U_LESS_THAN_EQUAL, true),
    (op::S_LESS_THAN_EQUAL, true),
    (op::F_ORD_EQUAL, true),
    (op::F_UNORD_NOT_EQUAL, true),
    (op::F_ORD_LESS_THAN, true),
    (op::F_ORD_GREATER_THAN, true),
    (op::F_ORD_LESS_THAN_EQUAL, true),
    (op::F_ORD_GREATER_THAN_EQUAL, true),
    (op::SHIFT_RIGHT_LOGICAL, true),
    (op::SHIFT_RIGHT_ARITHMETIC, true),
    (op::SHIFT_LEFT_LOGICAL, true),
    (op::BITWISE_OR, true),
    (op::BITWISE_XOR, true),
    (op::BITWISE_AND, true),
    (op::NOT, true),
    (op::BIT_FIELD_INSERT, true),
    (op::BIT_FIELD_S_EXTRACT, true),
    (op::BIT_FIELD_U_EXTRACT, true),
    (op::BIT_REVERSE, true),
    (op::BIT_COUNT, true),
    (op::DPDX, true),
    (op::DPDY, true),
    (op::FWIDTH, true),
    (op::DPDX_FINE, true),
    (op::DPDY_FINE, true),
    (op::FWIDTH_FINE, true),
    (op::DPDX_COARSE, true),
    (op::DPDY_COARSE, true),
    (op::FWIDTH_COARSE, true),
    (op::ATOMIC_LOAD, true),
    (op::ATOMIC_STORE, false),
    (op::ATOMIC_EXCHANGE, true),
    (op::ATOMIC_COMPARE_EXCHANGE, true),
    (op::ATOMIC_I_ADD, true),
    (op::ATOMIC_I_SUB, true),
    (op::ATOMIC_S_MIN, true),
    (op::ATOMIC_U_MIN, true),
    (op::ATOMIC_S_MAX, true),
    (op::ATOMIC_U_MAX, true),
    (op::ATOMIC_AND, true),
    (op::ATOMIC_OR, true),
    (op::ATOMIC_XOR, true),
];

/// The five words before the first instruction: the magic number, the
/// version, the generator, the id bound and the schema.
const HEADER: usize = 5;

/// The disassembly of `words`, the module a shader's SPIR-V is.
#[must_use]
pub fn disassemble(words: &[u32]) -> String {
    let mut out = String::new();
    if words.first() != Some(&MAGIC) {
        return format!("; not a SPIR-V module: {} words\n{out}", words.len());
    }
    let _ = writeln!(
        out,
        "; SPIR-V {}.{} generator {:#x}, bound {}, schema {}",
        (words[1] >> 16) & 0xff,
        (words[1] >> 8) & 0xff,
        words[2],
        words[3],
        words[4],
    );
    let mut at = HEADER;
    while at < words.len() {
        let head = words[at];
        let count = (head >> 16) as usize;
        let opcode = (head & 0xffff) as u16;
        if count == 0 || at + count > words.len() {
            let _ = writeln!(out, "; word {at} is not a whole instruction");
            return out;
        }
        let operands = &words[at + 1..at + count];
        if opcode != op::NAME && opcode != op::MEMBER_NAME && opcode != op::ENTRY_POINT {
            let _ = writeln!(out, "                ; word {at}");
        }
        if let Some((_, typed)) = RESULTS.iter().find(|(code, _)| *code == opcode) {
            let result_id = if *typed {
                operands.get(1)
            } else {
                operands.first()
            };
            if let Some(id) = result_id {
                let _ = write!(out, "%{id} = ");
            }
        }
        let _ = write!(out, "{}", name(opcode));
        for operand in operands {
            let _ = write!(out, " {operand}");
        }
        out.push('\n');
        at += count;
    }
    out
}

fn name(opcode: u16) -> &'static str {
    OPCODES
        .iter()
        .find(|(code, _)| *code == opcode)
        .map_or("Op?", |(_, name)| *name)
}
