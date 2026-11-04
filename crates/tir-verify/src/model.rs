use tir_diag::Span;
use tir_types::Type;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Function {
    pub parameters: Vec<Type>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub label: String,
    pub parameters: Vec<ValueDefinition>,
    pub instructions: Vec<Instruction>,
    pub terminator: Option<Terminator>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValueDefinition {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValueUse {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Instruction {
    pub opcode: String,
    pub results: Vec<ValueDefinition>,
    pub operands: Vec<ValueUse>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edge {
    pub target: String,
    pub arguments: Vec<ValueUse>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminatorKind {
    Branch,
    ConditionalBranch,
    SwitchInteger,
    SwitchEnum,
    IndirectBranch,
    Return,
    Fail,
    TailCall,
    Unreachable,
    Trap,
    TryCall,
    Invoke,
    CoroutineYield,
    CoroutineResume,
    CoroutineFinish,
    Resume,
    Throw,
    LongJump,
    AssemblyGoto,
    Deopt,
    IgnoreHit,
    AcceptHit,
}

impl TerminatorKind {
    #[must_use]
    pub const fn forbidden_in_structured_control_flow(self) -> bool {
        matches!(
            self,
            Self::IndirectBranch | Self::Invoke | Self::Resume | Self::Throw | Self::LongJump
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Terminator {
    pub kind: TerminatorKind,
    pub operands: Vec<ValueUse>,
    pub edges: Vec<Edge>,
    pub span: Span,
}
