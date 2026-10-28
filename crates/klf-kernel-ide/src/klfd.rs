//! `.klfd` files: a definition package compiled whole, or a loose file
//! parsed alone, with an outline of its declarations either way.

use crate::{Builder, PACKAGE_MARKER};
use klf_core::FileId;
use klf_diag::Diagnostic;
use klf_driver::{Backend, LanguageDefinition, load_package};
use klf_facts::{Entity, Symbol, SymbolKind, TokenClass, TokenModifier};
use klf_kira_syntax::ast::{
    CMember, DeclKind, EnumItem, FunctionDecl, Ident, Member, MemberKind, Param, TraitItem,
};
use klf_services::{DefinitionPackage, DefinitionSource, Session};
use klf_source::{SourceFile, Span};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

/// Compiles the package at `root` with the open documents in place of their
/// files, the way `klf check` does.
pub(crate) fn package(builder: &mut Builder<'_>, root: &Path) {
    let package = match load_package(root) {
        Ok(package) => package,
        Err(error) => {
            let file = builder.file(&root.join(PACKAGE_MARKER));
            builder.error(
                file,
                0,
                0,
                "KLF-DEF-0001",
                format!("cannot load definition package: {error}"),
            );
            return;
        }
    };
    let mut sources = Vec::new();
    let mut ids: HashMap<String, FileId> = HashMap::new();
    for source in package.sources() {
        let path = root.join(&source.name);
        let text = builder
            .text(&path)
            .unwrap_or_else(|| source.text.to_string());
        let file = builder.file(&path);
        ids.insert(source.name.clone(), file);
        outline(
            builder,
            &SourceFile::new(file, source.name.as_str(), text.as_str()),
        );
        sources.push(DefinitionSource::new(source.name.clone(), text));
    }
    let Ok(target) = klf_driver::host_triple().parse::<klf_target::Target>() else {
        return;
    };
    let mut session = Session::new(target);
    let package = DefinitionPackage::from_sources(sources);
    let compiled = catch_unwind(AssertUnwindSafe(|| {
        LanguageDefinition::from_package(&mut session, &package, Backend::Vm).is_some()
    }));
    if compiled.is_err() {
        let file = builder.file(&root.join(PACKAGE_MARKER));
        builder.error(
            file,
            0,
            0,
            "KLF-DEF-0003",
            "the KLF Kira frontend stopped unexpectedly".to_owned(),
        );
    }
    let files: Vec<Option<FileId>> = session
        .files()
        .iter()
        .map(|file| ids.get(file.name().as_str()).copied())
        .collect();
    let lookup = |file: FileId| files.get(file.index()).copied().flatten();
    for diagnostic in session.diagnostics() {
        builder.report(diagnostic, &lookup);
    }
}

/// Parses one `.klfd` file outside any package.
pub(crate) fn loose(builder: &mut Builder<'_>, path: &Path) {
    let Some(text) = builder.text(path) else {
        return;
    };
    let file = builder.file(path);
    let name = path.to_string_lossy();
    let parsed = outline(
        builder,
        &SourceFile::new(file, name.as_ref(), text.as_str()),
    );
    for diagnostic in &parsed {
        builder.report(diagnostic, &|_| Some(file));
    }
}

/// Records the declarations of `source` as symbols and returns its parse
/// diagnostics.
fn outline(builder: &mut Builder<'_>, source: &SourceFile) -> Vec<Diagnostic> {
    let parsed = klf_kira_syntax::parse(source);
    let mut outline = Outline {
        builder,
        file: source.id(),
        text: source.text(),
    };
    for decl in &parsed.file.decls {
        outline.decl(&decl.kind, decl.span);
    }
    parsed.diagnostics
}

struct Outline<'b, 'd, 't> {
    builder: &'b mut Builder<'d>,
    file: FileId,
    text: &'t str,
}

impl Outline<'_, '_, '_> {
    fn decl(&mut self, kind: &DeclKind, span: Span) {
        match kind {
            DeclKind::Function(function) => {
                self.function(function, SymbolKind::Function, None);
            }
            DeclKind::Struct(decl) => {
                let owner = self.declare(&decl.name, span, SymbolKind::Struct, None);
                self.members(&decl.members, owner);
            }
            DeclKind::Enum(decl) => {
                let owner = self.declare(&decl.name, span, SymbolKind::Enum, None);
                for item in &decl.items {
                    match item {
                        EnumItem::Case(case) => {
                            self.declare(
                                &case.name,
                                case.span,
                                SymbolKind::EnumMember,
                                Some(owner),
                            );
                        }
                        EnumItem::Method(method) => {
                            self.function(method, SymbolKind::Method, Some(owner));
                        }
                    }
                }
            }
            DeclKind::Trait(decl) => {
                let owner = self.declare(&decl.name, span, SymbolKind::Trait, None);
                for item in &decl.items {
                    match item {
                        TraitItem::AssocType { name, .. } => {
                            self.declare(name, name.span, SymbolKind::TypeParameter, Some(owner));
                        }
                        TraitItem::Method(method) => {
                            self.function(method, SymbolKind::Method, Some(owner));
                        }
                    }
                }
            }
            DeclKind::Construct(decl) => {
                let owner = self.declare(&decl.name, span, SymbolKind::Class, None);
                for member in &decl.members {
                    match member {
                        CMember::Member(member) => self.member(member, Some(owner)),
                        CMember::ContentBlock { name, .. } => {
                            self.declare(name, name.span, SymbolKind::Property, Some(owner));
                        }
                        CMember::Requires(functions) => {
                            for function in functions {
                                self.function(function, SymbolKind::Method, Some(owner));
                            }
                        }
                    }
                }
            }
            DeclKind::Extend(decl) => {
                for member in &decl.items {
                    self.member(member, None);
                }
            }
            DeclKind::Alias(decl) => {
                self.declare(&decl.name, span, SymbolKind::TypeParameter, None);
            }
            DeclKind::Distinct(decl) => {
                self.declare(&decl.name, span, SymbolKind::Struct, None);
            }
            DeclKind::Constant(decl) => {
                self.declare(&decl.name, span, SymbolKind::Constant, None);
            }
        }
    }

    fn members(&mut self, members: &[Member], owner: Entity) {
        for member in members {
            self.member(member, Some(owner));
        }
    }

    fn member(&mut self, member: &Member, owner: Option<Entity>) {
        match &member.kind {
            MemberKind::Field { name, .. } => {
                self.declare(name, member.span, SymbolKind::Field, owner);
            }
            MemberKind::Method(method) => {
                self.function(method, SymbolKind::Method, owner);
            }
            MemberKind::Init { .. } => {}
        }
    }

    fn function(&mut self, function: &FunctionDecl, kind: SymbolKind, owner: Option<Entity>) {
        let entity = self.declare(&function.name, function.span, kind, owner);
        let parameters = function
            .params
            .iter()
            .filter_map(|param| match param {
                Param::Named(named) => Some(self.slice(named.span).to_owned()),
                Param::SelfParam { .. } => None,
            })
            .collect();
        self.builder.facts.declare_symbol(
            entity,
            Symbol {
                parameters,
                ..self.symbol(&function.name, function.span, kind, owner)
            },
        );
    }

    fn declare(
        &mut self,
        name: &Ident,
        span: Span,
        kind: SymbolKind,
        owner: Option<Entity>,
    ) -> Entity {
        let entity = Entity(u64::from(self.file.as_u32()) << 32 | u64::from(name.span.start));
        let symbol = self.symbol(name, span, kind, owner);
        self.builder.facts.declare_symbol(entity, symbol);
        let class = match kind {
            SymbolKind::Function => TokenClass::Function,
            SymbolKind::Method => TokenClass::Method,
            SymbolKind::Struct => TokenClass::Struct,
            SymbolKind::Enum => TokenClass::Enum,
            SymbolKind::Trait => TokenClass::Interface,
            SymbolKind::Class => TokenClass::Class,
            SymbolKind::EnumMember => TokenClass::EnumMember,
            SymbolKind::Constant => TokenClass::Constant,
            SymbolKind::Field | SymbolKind::Property => TokenClass::Property,
            _ => TokenClass::Type,
        };
        self.builder.facts.classify(
            self.file,
            name.span.start,
            name.span.end,
            class,
            TokenModifier::Declaration.bit(),
        );
        entity
    }

    fn symbol(&self, name: &Ident, span: Span, kind: SymbolKind, owner: Option<Entity>) -> Symbol {
        let signature = self
            .slice(span)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .trim_end_matches('{')
            .trim_end()
            .to_owned();
        Symbol {
            name: name.name.as_str().to_owned(),
            kind: Some(kind),
            file: Some(self.file),
            start: span.start,
            end: span.end,
            selection_start: name.span.start,
            selection_end: name.span.end,
            container: owner,
            signature,
            ..Symbol::default()
        }
    }

    fn slice(&self, span: Span) -> &str {
        self.text
            .get(span.start as usize..span.end as usize)
            .unwrap_or_default()
    }
}
