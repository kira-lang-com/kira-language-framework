//! The KLF Kira abstract syntax tree (Appendix A.1).
//!
//! Every node carries its source [`Span`]. Identifiers hold an interned
//! [`Name`]; literals keep their source text (integers, floats) or decoded
//! value (strings) for later lowering.

use klf_core::Name;
use klf_source::Span;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ident {
    pub name: Name,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Path {
    pub segments: Vec<Ident>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct File {
    pub imports: Vec<Import>,
    pub decls: Vec<Decl>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Import {
    pub path: Path,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Annotation {
    pub path: Path,
    pub args: AnnotationArgs,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnnotationArgs {
    None,
    Call(Vec<Arg>),
    Fields(Vec<(Ident, Expr)>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decl {
    pub annotations: Vec<Annotation>,
    pub kind: DeclKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeclKind {
    Function(FunctionDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Trait(TraitDecl),
    Construct(ConstructDecl),
    Extend(ExtendDecl),
    Alias(AliasDecl),
    Distinct(DistinctDecl),
    Constant(ConstantDecl),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GParam {
    pub name: Ident,
    pub bounds: Vec<Type>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Borrow,
    BorrowMut,
    Move,
    Copy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Param {
    Named(Box<NamedParam>),
    SelfParam { mode: Option<Mode>, span: Span },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedParam {
    pub label: Option<Ident>,
    pub name: Ident,
    pub mode: Option<Mode>,
    pub ty: Type,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionDecl {
    pub name: Ident,
    pub generics: Vec<GParam>,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub body: Option<Block>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructDecl {
    pub name: Ident,
    pub generics: Vec<GParam>,
    pub conforms: Vec<Type>,
    pub members: Vec<Member>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Member {
    pub annotations: Vec<Annotation>,
    pub kind: MemberKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemberKind {
    Field {
        is_var: bool,
        name: Ident,
        ty: Type,
        init: Option<FieldInit>,
    },
    Method(FunctionDecl),
    Init {
        params: Vec<Param>,
        body: Block,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldInit {
    Expr(Expr),
    Block(Block),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumDecl {
    pub name: Ident,
    pub generics: Vec<GParam>,
    pub conforms: Vec<Type>,
    pub items: Vec<EnumItem>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnumItem {
    Case(Case),
    Method(FunctionDecl),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Case {
    pub name: Ident,
    pub payload: CasePayload,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CasePayload {
    None,
    Tuple(Vec<Type>),
    Typed { ty: Type, default: Option<Expr> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraitDecl {
    pub name: Ident,
    pub generics: Vec<GParam>,
    pub conforms: Vec<Type>,
    pub items: Vec<TraitItem>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TraitItem {
    AssocType { name: Ident, bound: Option<Type> },
    Method(FunctionDecl),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstructDecl {
    pub name: Ident,
    pub generics: Vec<GParam>,
    pub params: Option<Vec<Param>>,
    pub extends: Vec<Type>,
    pub members: Vec<CMember>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CMember {
    Member(Box<Member>),
    ContentBlock { name: Ident, body: Block },
    Requires(Vec<FunctionDecl>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtendDecl {
    pub ty: Type,
    pub conforms: Vec<Type>,
    pub items: Vec<Member>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AliasDecl {
    pub name: Ident,
    pub generics: Vec<GParam>,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DistinctDecl {
    pub name: Ident,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstantDecl {
    pub name: Ident,
    pub ty: Option<Type>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeKind {
    Named { path: Path, args: Vec<Type> },
    Array(Box<Type>),
    Tuple(Vec<Type>),
    Function { params: Vec<Type>, ret: Box<Type> },
    Any(Box<Type>),
    Some(Box<Type>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StmtKind {
    Let {
        is_var: bool,
        name: Ident,
        ty: Option<Type>,
        value: Expr,
    },
    Assign {
        target: Expr,
        op: AssignOp,
        value: Expr,
    },
    Expr(Expr),
    If {
        branches: Vec<(Expr, Block)>,
        else_block: Option<Block>,
    },
    While {
        cond: Expr,
        body: Block,
    },
    For {
        var: Ident,
        iter: Expr,
        body: Block,
    },
    Match {
        scrutinee: Expr,
        arms: Vec<MatchArm>,
    },
    Attempt {
        body: Block,
        handlers: Vec<MatchArm>,
    },
    Return(Option<Expr>),
    Break,
    Continue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

/// What a match arm tests its subject against.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatternKind {
    /// `Case` or `Case(a, b)`: an enum case, binding its payload.
    Case { name: Ident, bindings: Vec<Ident> },
    /// `"a"` or `"a", "b"`: a `String` subject equal to any of the texts.
    Text(Vec<String>),
    /// `else`: a `String` subject equal to none of the other arms' texts.
    Otherwise,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignOp {
    Assign,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExprKind {
    /// `try operand`: the `Ok` payload of a Result-shaped value, or an early
    /// return of its `Error`.
    Try(Box<Expr>),
    Int(Name),
    Float(Name),
    Str(String),
    Bool(bool),
    Path(Path),
    EnumLit {
        name: Ident,
        args: Option<Vec<Arg>>,
    },
    Tuple(Vec<Expr>),
    StructLit {
        ty: Type,
        fields: Vec<(Ident, Expr)>,
    },
    ArrayLit(Vec<Expr>),
    MapLit(Vec<(Expr, Expr)>),
    Closure {
        params: Vec<Ident>,
        body: Vec<Stmt>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    As {
        expr: Box<Expr>,
        ty: Type,
    },
    Is {
        expr: Box<Expr>,
        ty: Type,
    },
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        otherwise: Box<Expr>,
    },
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
    },
    Field {
        base: Box<Expr>,
        name: Ident,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Arg {
    pub label: Option<Ident>,
    pub value: Expr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    Move,
    Copy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinOp {
    Or,
    And,
    BitOr,
    BitXor,
    BitAnd,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Shl,
    Shr,
    Add,
    Sub,
    WrapAdd,
    WrapSub,
    Mul,
    Div,
    Rem,
    WrapMul,
}
