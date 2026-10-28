//! Every VM opcode sits in the slot section 15.5 assigns it: opcodes are
//! numbered in order from the start of their range, as the table lists them.

use klf_vm::Opcode;

/// The section 15.5 table: each range's first opcode and its mnemonics, in
/// order.
const RANGES: &[(u8, &[&str])] = &[
    (
        0x00,
        &[
            "NOP", "MOV", "MOV2", "LOADK", "LOADI", "LOADNULL", "LEAF", "LEAG", "LEAFN", "LEAD",
            "LEAW", "SELECT",
        ],
    ),
    (
        0x10,
        &[
            "ADD", "SUB", "MUL", "SDIV", "UDIV", "SREM", "UREM", "AND", "OR", "XOR", "NOT", "NEG",
            "SHL", "LSHR", "ASHR", "ROTL", "ROTR", "ADDO", "SUBO", "MULO", "CTLZ", "CTTZ", "CTPOP",
            "BSWAP", "BITREV", "ICMP", "ADDI", "MULI",
        ],
    ),
    (
        0x30,
        &[
            "FADD", "FSUB", "FMUL", "FDIV", "FREM", "FNEG", "FABS", "FSQRT", "FMIN", "FMAX",
            "FFLOOR", "FCEIL", "FTRUNC", "FROUND", "FMA", "FCMP",
        ],
    ),
    (
        0x48,
        &[
            "TRUNC", "ZEXT", "SEXT", "FPTRUNC", "FPEXT", "SITOFP", "UITOFP", "FPTOSI", "FPTOUI",
            "BITCAST",
        ],
    ),
    (
        0x58,
        &[
            "LOAD", "STORE", "MCPY", "MMOVE", "MSET", "PTRADD", "PTRDIFF", "ALLOCA", "HALLOCC",
            "HALLOCU", "HALLOCR", "HFREER", "RETAIN", "RELEASE", "WNEW", "WLOAD", "WRELEASE",
            "ISUNIQ", "CPYADDR", "MOVADDR", "DSTADDR", "ACCBEG", "ACCEND",
        ],
    ),
    (
        0x70,
        &[
            "JMP", "JIF", "JIFN", "SWITCH", "JIND", "CALL", "CALLI", "CALLC", "RET", "RETV",
            "FAIL", "TRAP", "UNREACH", "BR", "BR_IF", "BR_CMP",
        ],
    ),
    (
        0x80,
        &[
            "ALOAD", "ASTORE", "ARMW", "ACMPXCHG", "FENCE", "VASTART", "VAARG", "VACOPY", "VAEND",
            "SETJMP", "LONGJMP", "THROW", "RESUME", "LPAD",
        ],
    ),
];

fn slot(mnemonic: &str) -> u8 {
    for (start, names) in RANGES {
        if let Some(index) = names.iter().position(|name| *name == mnemonic) {
            return start + u8::try_from(index).expect("small range");
        }
    }
    panic!("`{mnemonic}` is not in section 15.5");
}

/// Every VM opcode and its section 15.5 mnemonic.
const OPCODES: &[(Opcode, &str)] = &[
    (Opcode::Nop, "NOP"),
    (Opcode::Mov, "MOV"),
    (Opcode::LoadK, "LOADK"),
    (Opcode::LoadI, "LOADI"),
    (Opcode::LoadNull, "LOADNULL"),
    (Opcode::GlobalA, "LEAG"),
    (Opcode::LeaFn, "LEAFN"),
    (Opcode::Select, "SELECT"),
    (Opcode::Add, "ADD"),
    (Opcode::Sub, "SUB"),
    (Opcode::Mul, "MUL"),
    (Opcode::SDiv, "SDIV"),
    (Opcode::UDiv, "UDIV"),
    (Opcode::SRem, "SREM"),
    (Opcode::URem, "UREM"),
    (Opcode::And, "AND"),
    (Opcode::Or, "OR"),
    (Opcode::Xor, "XOR"),
    (Opcode::Not, "NOT"),
    (Opcode::Neg, "NEG"),
    (Opcode::Shl, "SHL"),
    (Opcode::LShr, "LSHR"),
    (Opcode::AShr, "ASHR"),
    (Opcode::RotL, "ROTL"),
    (Opcode::RotR, "ROTR"),
    (Opcode::Ctlz, "CTLZ"),
    (Opcode::Cttz, "CTTZ"),
    (Opcode::Ctpop, "CTPOP"),
    (Opcode::Bswap, "BSWAP"),
    (Opcode::Bitrev, "BITREV"),
    (Opcode::Icmp, "ICMP"),
    (Opcode::FAdd, "FADD"),
    (Opcode::FSub, "FSUB"),
    (Opcode::FMul, "FMUL"),
    (Opcode::FDiv, "FDIV"),
    (Opcode::FRem, "FREM"),
    (Opcode::FNeg, "FNEG"),
    (Opcode::FAbs, "FABS"),
    (Opcode::FSqrt, "FSQRT"),
    (Opcode::FMin, "FMIN"),
    (Opcode::FMax, "FMAX"),
    (Opcode::FFloor, "FFLOOR"),
    (Opcode::FCeil, "FCEIL"),
    (Opcode::FTrunc, "FTRUNC"),
    (Opcode::FRound, "FROUND"),
    (Opcode::FFma, "FMA"),
    (Opcode::FCmp, "FCMP"),
    (Opcode::Trunc, "TRUNC"),
    (Opcode::ZExt, "ZEXT"),
    (Opcode::SExt, "SEXT"),
    (Opcode::FpTrunc, "FPTRUNC"),
    (Opcode::FpExt, "FPEXT"),
    (Opcode::SiToFp, "SITOFP"),
    (Opcode::UiToFp, "UITOFP"),
    (Opcode::FpToSi, "FPTOSI"),
    (Opcode::FpToUi, "FPTOUI"),
    (Opcode::Bitcast, "BITCAST"),
    (Opcode::Load, "LOAD"),
    (Opcode::Store, "STORE"),
    (Opcode::MemCpy, "MCPY"),
    (Opcode::MemMove, "MMOVE"),
    (Opcode::MemSet, "MSET"),
    (Opcode::PtrAdd, "PTRADD"),
    (Opcode::PtrDiff, "PTRDIFF"),
    (Opcode::Alloca, "ALLOCA"),
    (Opcode::HallocC, "HALLOCC"),
    (Opcode::HallocU, "HALLOCU"),
    (Opcode::HallocR, "HALLOCR"),
    (Opcode::HfreeR, "HFREER"),
    (Opcode::Retain, "RETAIN"),
    (Opcode::Release, "RELEASE"),
    (Opcode::WeakLoad, "WLOAD"),
    (Opcode::IsUnique, "ISUNIQ"),
    (Opcode::AccBegin, "ACCBEG"),
    (Opcode::AccEnd, "ACCEND"),
    (Opcode::Jmp, "JMP"),
    (Opcode::Jif, "JIF"),
    (Opcode::Jifn, "JIFN"),
    (Opcode::Call, "CALL"),
    (Opcode::Ret, "RET"),
    (Opcode::RetV, "RETV"),
    (Opcode::Fail, "FAIL"),
    (Opcode::Trap, "TRAP"),
    (Opcode::Unreach, "UNREACH"),
    (Opcode::Br, "BR"),
    (Opcode::BrIf, "BR_IF"),
    (Opcode::BrCmp, "BR_CMP"),
    (Opcode::ALoad, "ALOAD"),
    (Opcode::AStore, "ASTORE"),
    (Opcode::ARmw, "ARMW"),
    (Opcode::ACas, "ACMPXCHG"),
    (Opcode::Fence, "FENCE"),
    (Opcode::SetJmp, "SETJMP"),
    (Opcode::LongJmp, "LONGJMP"),
];

#[test]
fn every_opcode_sits_in_its_section_15_5_slot() {
    for &(opcode, mnemonic) in OPCODES {
        assert_eq!(opcode as u8, slot(mnemonic), "{mnemonic}");
        assert_eq!(Opcode::from_u8(opcode as u8), Some(opcode), "{mnemonic}");
    }
    let known = (0..=u8::MAX)
        .filter(|byte| Opcode::from_u8(*byte).is_some())
        .count();
    assert_eq!(known, OPCODES.len(), "an opcode outside section 15.5");
}
