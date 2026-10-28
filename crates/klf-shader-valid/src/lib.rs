//! Validation of KSIR modules (`KLF-KSL.md` section 16): rules SV1 to SV24,
//! the module information passes and backends read, uniformity and constant
//! evaluation.
//!
//! KSIR arithmetic is bit-exact over fixed-width integers and floats, so the
//! narrowing and sign casts are deliberate; the printers, parser and type
//! resolver dispatch over every expression kind in one match each.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    clippy::match_same_arms,
    clippy::float_cmp
)]

mod capabilities;
mod consts;
mod function;
mod info;
mod interface;
mod stages;
mod types;
mod uniformity;
mod walk;

pub use capabilities::used_capabilities;
pub use consts::{ConstValue, Evaluator, Scalar as ConstScalar};
pub use info::{ExpressionInfo, FunctionInfo, GlobalUse, ModuleInfo};

use klf_shader::{Module, Span};

/// A rule a module broke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    /// The rule, `SV1` to `SV24`.
    pub rule: &'static str,
    /// The user-facing key of section 16.4, when the failure is the
    /// program's rather than the definition's.
    pub key: Option<&'static str>,
    pub message: String,
    pub span: Span,
    /// Whether this is only a warning (uniformity without WGSL).
    pub warning: bool,
}

impl ValidationError {
    #[must_use]
    pub fn new(rule: &'static str, message: impl Into<String>, span: Span) -> Self {
        Self {
            rule,
            key: None,
            message: message.into(),
            span,
            warning: false,
        }
    }

    #[must_use]
    pub fn keyed(
        rule: &'static str,
        key: &'static str,
        message: impl Into<String>,
        span: Span,
    ) -> Self {
        Self {
            rule,
            key: Some(key),
            message: message.into(),
            span,
            warning: false,
        }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.rule, self.message)
    }
}

/// What validation needs to know beyond the module.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Whether WGSL is among the requested targets, which makes uniformity
    /// failures errors (section 16.5).
    pub wgsl: bool,
}

/// The outcome: the module information when no error was found, and every
/// error and warning in order.
#[derive(Debug, Default)]
pub struct Validation {
    pub info: Option<ModuleInfo>,
    pub problems: Vec<ValidationError>,
}

impl Validation {
    pub fn errors(&self) -> impl Iterator<Item = &ValidationError> {
        self.problems.iter().filter(|problem| !problem.warning)
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.errors().next().is_none()
    }
}

/// Validates `module`.
#[must_use]
pub fn validate(module: &Module, options: Options) -> Validation {
    let mut problems = Vec::new();
    types::check_types(module, &mut problems);
    if !problems.is_empty() {
        return Validation {
            info: None,
            problems,
        };
    }
    let constants = consts::check_module(module, &mut problems);
    types::check_globals(module, &mut problems);
    let mut info = ModuleInfo {
        global_types: Vec::new(),
        constants,
        functions: Vec::new(),
        entries: Vec::new(),
    };
    match klf_shader::resolve::resolve_globals(module) {
        Ok(resolved) => info.global_types = resolved,
        Err((handle, message)) => problems.push(ValidationError::new(
            "SV6",
            message,
            module.global_expressions.span(handle),
        )),
    }
    for (handle, function) in module.functions.iter() {
        let span = module.functions.span(handle);
        info.functions.push(function::check_function(
            module,
            &info,
            function,
            None,
            span,
            &mut problems,
        ));
    }
    for entry in &module.entry_points {
        info.entries.push(function::check_function(
            module,
            &info,
            &entry.function,
            Some(entry.stage),
            entry.span,
            &mut problems,
        ));
    }
    interface::check_interfaces(module, &mut problems);
    interface::check_shaders(module, &mut problems);
    stages::check_stages(module, &mut info, options, &mut problems);
    let valid = problems.iter().all(|problem| problem.warning);
    Validation {
        info: valid.then_some(info),
        problems,
    }
}
