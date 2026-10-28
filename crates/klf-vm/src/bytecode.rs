//! `.klbc` instruction encoding of section 15.4 and 15.5.

/// One decoded bytecode instruction. Every instruction is one 64-bit word:
/// opcode in bits 0-7, flags 8-15, operand A 16-31, B 32-47, C 48-63.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Instr {
    pub op: Opcode,
    pub flags: u8,
    pub a: u16,
    pub b: u16,
    pub c: u16,
}

impl Instr {
    #[must_use]
    pub fn abc(op: Opcode, a: u16, b: u16, c: u16) -> Self {
        Self {
            op,
            flags: 0,
            a,
            b,
            c,
        }
    }

    #[must_use]
    pub fn with_flags(mut self, flags: u8) -> Self {
        self.flags = flags;
        self
    }

    /// Operands B and C read together as a signed 32-bit immediate (`ABx`).
    #[must_use]
    pub fn bx(self) -> i32 {
        (u32::from(self.b) | (u32::from(self.c) << 16)) as i32
    }

    #[must_use]
    pub fn from_bx(op: Opcode, a: u16, imm: i32) -> Self {
        let bits = imm as u32;
        Self {
            op,
            flags: 0,
            a,
            #[allow(clippy::cast_possible_truncation)]
            b: bits as u16,
            #[allow(clippy::cast_possible_truncation)]
            c: (bits >> 16) as u16,
        }
    }

    #[must_use]
    pub fn encode(self) -> u64 {
        u64::from(self.op as u8)
            | (u64::from(self.flags) << 8)
            | (u64::from(self.a) << 16)
            | (u64::from(self.b) << 32)
            | (u64::from(self.c) << 48)
    }

    /// Decode one code word. Returns `None` for an unknown opcode.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn decode(word: u64) -> Option<Self> {
        Some(Self {
            op: Opcode::from_u8(word as u8)?,
            flags: (word >> 8) as u8,
            a: (word >> 16) as u16,
            b: (word >> 32) as u16,
            c: (word >> 48) as u16,
        })
    }
}

/// Width code carried in arithmetic flags: 0->8, 1->16, 2->32, 3->64, 4->128.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WidthCode {
    W8,
    W16,
    W32,
    W64,
    W128,
}

impl WidthCode {
    #[must_use]
    pub fn from_bits(bits: u16) -> Option<Self> {
        Some(match bits {
            // A boolean (`i1`) occupies a byte lane in registers and memory.
            1 | 8 => Self::W8,
            16 => Self::W16,
            32 => Self::W32,
            64 => Self::W64,
            128 => Self::W128,
            _ => return None,
        })
    }

    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Self::W8 => 0,
            Self::W16 => 1,
            Self::W32 => 2,
            Self::W64 => 3,
            Self::W128 => 4,
        }
    }

    #[must_use]
    pub fn bits(self) -> u32 {
        match self {
            Self::W8 => 8,
            Self::W16 => 16,
            Self::W32 => 32,
            Self::W64 => 64,
            Self::W128 => 128,
        }
    }
}

/// Overflow mode carried in arithmetic flags bits 4-5.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverflowMode {
    Wrap,
    Trap,
    Saturate,
}

impl OverflowMode {
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Self::Wrap => 0,
            Self::Trap => 1,
            Self::Saturate => 2,
        }
    }

    #[must_use]
    pub fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Trap,
            2 => Self::Saturate,
            _ => Self::Wrap,
        }
    }
}

/// Build the flags byte for an arithmetic instruction.
#[must_use]
pub fn arith_flags(width: WidthCode, signed: bool, overflow: OverflowMode) -> u8 {
    width.code() | (u8::from(signed) << 3) | (overflow.code() << 4)
}

#[must_use]
pub fn flags_width(flags: u8) -> Option<WidthCode> {
    Some(match flags & 0b111 {
        0 => WidthCode::W8,
        1 => WidthCode::W16,
        2 => WidthCode::W32,
        3 => WidthCode::W64,
        4 => WidthCode::W128,
        _ => return None,
    })
}

#[must_use]
pub fn flags_signed(flags: u8) -> bool {
    flags & 0b1000 != 0
}

#[must_use]
pub fn flags_overflow(flags: u8) -> OverflowMode {
    OverflowMode::from_code((flags >> 4) & 0b11)
}

macro_rules! opcodes {
    ($($name:ident = $value:expr),+ $(,)?) => {
        /// Bytecode opcodes of section 15.5, in their assigned numeric slots.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u8)]
        pub enum Opcode {
            $($name = $value),+
        }

        impl Opcode {
            #[must_use]
            pub fn from_u8(value: u8) -> Option<Self> {
                match value {
                    $($value => Some(Self::$name),)+
                    _ => None,
                }
            }
        }
    };
}

opcodes! {
    Nop = 0x00,
    Mov = 0x01,
    LoadK = 0x03,
    LoadI = 0x04,
    LoadNull = 0x05,
    GlobalA = 0x07,
    LeaFn = 0x08,
    Select = 0x0B,
    Add = 0x10,
    Sub = 0x11,
    Mul = 0x12,
    SDiv = 0x13,
    UDiv = 0x14,
    SRem = 0x15,
    URem = 0x16,
    And = 0x17,
    Or = 0x18,
    Xor = 0x19,
    Not = 0x1A,
    Neg = 0x1B,
    Shl = 0x1C,
    LShr = 0x1D,
    AShr = 0x1E,
    RotL = 0x1F,
    RotR = 0x20,
    Ctlz = 0x24,
    Cttz = 0x25,
    Ctpop = 0x26,
    Bswap = 0x27,
    Bitrev = 0x28,
    Icmp = 0x29,
    FAdd = 0x30,
    FSub = 0x31,
    FMul = 0x32,
    FDiv = 0x33,
    FRem = 0x34,
    FNeg = 0x35,
    FAbs = 0x36,
    FSqrt = 0x37,
    FMin = 0x38,
    FMax = 0x39,
    FFloor = 0x3A,
    FCeil = 0x3B,
    FTrunc = 0x3C,
    FRound = 0x3D,
    FFma = 0x3E,
    FCmp = 0x3F,
    Trunc = 0x48,
    ZExt = 0x49,
    SExt = 0x4A,
    FpTrunc = 0x4B,
    FpExt = 0x4C,
    SiToFp = 0x4D,
    UiToFp = 0x4E,
    FpToSi = 0x4F,
    FpToUi = 0x50,
    Bitcast = 0x51,
    Load = 0x58,
    Store = 0x59,
    MemCpy = 0x5A,
    MemMove = 0x5B,
    MemSet = 0x5C,
    PtrAdd = 0x5D,
    PtrDiff = 0x5E,
    Alloca = 0x5F,
    HallocC = 0x60,
    HallocU = 0x61,
    HallocR = 0x62,
    HfreeR = 0x63,
    Retain = 0x64,
    Release = 0x65,
    WeakLoad = 0x67,
    IsUnique = 0x69,
    AccBegin = 0x6D,
    AccEnd = 0x6E,
    Jmp = 0x70,
    Jif = 0x71,
    Jifn = 0x72,
    Call = 0x75,
    Ret = 0x78,
    RetV = 0x79,
    Fail = 0x7A,
    Trap = 0x7B,
    Unreach = 0x7C,
    Br = 0x7D,
    BrIf = 0x7E,
    BrCmp = 0x7F,
    ALoad = 0x80,
    AStore = 0x81,
    ARmw = 0x82,
    ACas = 0x83,
    Fence = 0x84,
    SetJmp = 0x89,
    LongJmp = 0x8A,
}
