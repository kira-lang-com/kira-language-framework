//! The closed vocabularies facts are recorded in (`KLF-EDITOR.md` sections 6
//! and 12). Each value has a fixed code: the code is what crosses the service
//! ABI, and the order is the semantic token legend's, so codes never change.

macro_rules! coded {
    ($(#[$meta:meta])* $name:ident { $($variant:ident),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum $name {
            $($variant),*
        }

        impl $name {
            /// Every value, in code order.
            pub const ALL: &[Self] = &[$(Self::$variant),*];

            #[must_use]
            pub const fn code(self) -> u32 {
                self as u32
            }

            #[must_use]
            pub fn from_code(code: u64) -> Option<Self> {
                usize::try_from(code).ok().and_then(|index| Self::ALL.get(index).copied())
            }

            #[must_use]
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => stringify!($variant)),*
                }
            }

            #[must_use]
            pub fn from_name(name: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|value| value.name() == name)
            }
        }
    };
}

coded!(
    /// What a token or name occurrence is (section 12.1).
    TokenClass {
        Comment, DocComment, String, Character, Escape, Number, Boolean, Keyword, Operator,
        Punctuation, Namespace, Type, Struct, Class, Enum, Interface, TypeParameter, Function,
        Method, Macro, Variable, Parameter, Property, EnumMember, Constant, Label, Annotation,
        Builtin, Error,
    }
);

coded!(
    /// A refinement of a token class (section 12.1). Recorded as a bit set.
    TokenModifier {
        Declaration, Definition, Readonly, Static, Deprecated, Abstract, Async, Modification,
        Documentation, DefaultLibrary, Mutable, Consuming, Borrowed, Unmanaged, Control,
        Contextual, Injected, Inactive, Unresolved, Resource, StageIo, Access, Path,
    }
);

impl TokenModifier {
    #[must_use]
    pub const fn bit(self) -> u32 {
        1 << self.code()
    }

    /// The modifiers a bit set names, in code order.
    #[must_use]
    pub fn of_bits(bits: u32) -> Vec<Self> {
        Self::ALL
            .iter()
            .copied()
            .filter(|modifier| bits & modifier.bit() != 0)
            .collect()
    }
}

coded!(
    /// What a declared symbol is (section 6.4). The first 26 are LSP's, in
    /// LSP's order, so `code() + 1` is the LSP number for those.
    SymbolKind {
        File, Module, Namespace, Package, Class, Method, Property, Field, Constructor, Enum,
        Interface, Function, Variable, Constant, String, Number, Boolean, Array, Object, Key,
        Null, EnumMember, Struct, Event, Operator, TypeParameter, Macro, Trait, Resource, Stage,
        Label, Test,
    }
);

impl SymbolKind {
    /// The LSP `SymbolKind` number (section 12.4 for the kinds LSP lacks).
    #[must_use]
    pub const fn lsp(self) -> u32 {
        match self {
            Self::Macro | Self::Stage => 12,
            Self::Trait => 11,
            Self::Resource => 13,
            Self::Label => 20,
            Self::Test => 6,
            other => other.code() + 1,
        }
    }
}

coded!(
    /// How a reference uses what it names (section 6.5).
    ReferenceRole { Read, Write, ReadWrite, Call, Type, Import, Implementation, Other }
);

coded!(
    /// What an inlay hint shows (section 6.6).
    HintKind { Type, Parameter, Ownership, Layout, Value, Other }
);

coded!(
    /// What a folding range covers (section 6.7).
    FoldKind { Region, Comment, Imports, Other }
);

coded!(
    /// What a runnable runs (section 6.8).
    RunnableKind { Main, Test, Benchmark, Corpus, Shader }
);

coded!(
    /// The phase that recorded a fact. A later phase's token class wins.
    FactPhase { Units, Parse, Elaborate, Lower, Kernel }
);

coded!(
    /// Whether a session builds or serves an editor (section 6.2). Recording
    /// services do nothing in `Build`.
    AnalysisMode { Build, Editor }
);

#[allow(clippy::derivable_impls)]
impl Default for AnalysisMode {
    fn default() -> Self {
        Self::Build
    }
}
