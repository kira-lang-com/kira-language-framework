//! What the shader driver reports: a kernel key mapped to the language's own
//! code (section 16.4), an internal error that blames the definition
//! (`KLF-SHD-9xxx`), or a driver error (`KLF-SHD-0xxx`), rendered as text or
//! as one JSON object per line (`KLF-RIDL.md` section 31.4).

use klf_diag::Severity;
use klf_services::{DiagnosticSpec, Session};
use klf_shader::{Module, Span, Target};
use klf_shader_valid::ValidationError;
use std::fmt::Write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub warning: bool,
    pub code: String,
    pub message: String,
    /// The file, line and column, when the problem has a place.
    pub at: Option<(String, u32, u32)>,
    /// The file and byte range, which is what an editor needs.
    pub span: Option<(String, u32, u32)>,
    /// The target that refused it; `None` for a problem every target shares.
    pub target: Option<Target>,
}

impl Problem {
    #[must_use]
    pub fn driver(code: &str, message: impl Into<String>) -> Self {
        Self {
            warning: false,
            code: code.to_owned(),
            message: message.into(),
            at: None,
            span: None,
            target: None,
        }
    }

    /// The same problem, attributed to `target`.
    #[must_use]
    pub fn for_target(mut self, target: Target) -> Self {
        self.target = Some(target);
        self
    }

    #[must_use]
    pub fn render(&self, json: bool) -> String {
        let severity = if self.warning { "warning" } else { "error" };
        if json {
            let mut out = format!(
                "{{\"severity\":\"{severity}\",\"code\":\"{}\",\"message\":\"{}\"",
                escape(&self.code),
                escape(&self.message)
            );
            if let Some((file, line, column)) = &self.at {
                let _ = write!(
                    out,
                    ",\"file\":\"{}\",\"line\":{line},\"column\":{column}",
                    escape(file)
                );
            }
            out.push('}');
            return out;
        }
        if self.code.is_empty() {
            return self.message.clone();
        }
        match &self.at {
            Some((file, line, column)) => format!(
                "{file}:{line}:{column}: {severity}[{}]: {}",
                self.code, self.message
            ),
            None => format!("{severity}[{}]: {}", self.code, self.message),
        }
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out
}

/// How validation and backend failures become problems: through the
/// language's key map and diagnostic registry.
pub struct Reporter<'a> {
    pub lang: &'a str,
    pub keys: &'a [(String, i64)],
    pub specs: &'a [DiagnosticSpec],
}

impl Reporter<'_> {
    /// What the definition reported while lowering; when it reported nothing
    /// the Session kept, its failure as the phase rendered it.
    #[must_use]
    pub fn definition(&self, session: &Session, rendered: String) -> Vec<Problem> {
        let place = |span: klf_source::Span| {
            let file = session.file(span.file);
            let position = file.line_column(span.start);
            (
                file.name().as_str().to_owned(),
                position.line,
                position.column,
            )
        };
        let reported = session
            .runtime_diagnostics()
            .iter()
            .map(|diagnostic| Problem {
                warning: diagnostic.severity != Severity::Error,
                code: diagnostic.code.to_string(),
                message: format!("{}: {}", diagnostic.title, diagnostic.explanation),
                at: Some(place(diagnostic.span)),
                span: Some((
                    session
                        .file(diagnostic.span.file)
                        .name()
                        .as_str()
                        .to_owned(),
                    diagnostic.span.start,
                    diagnostic.span.end,
                )),
                target: None,
            });
        let kernel = session
            .diagnostics()
            .iter()
            .filter(|diagnostic| matches!(diagnostic.severity, Severity::Error | Severity::Warning))
            .map(|diagnostic| Problem {
                warning: diagnostic.severity == Severity::Warning,
                code: diagnostic.code.to_owned(),
                message: diagnostic.message.clone(),
                at: diagnostic.primary_span.map(place),
                span: diagnostic.primary_span.map(|span| {
                    (
                        session.file(span.file).name().as_str().to_owned(),
                        span.start,
                        span.end,
                    )
                }),
                target: None,
            });
        let problems: Vec<Problem> = kernel.chain(reported).collect();
        if problems.is_empty() {
            return vec![Problem {
                warning: false,
                code: String::new(),
                message: rendered,
                at: None,
                span: None,
                target: None,
            }];
        }
        problems
    }

    /// The byte range of a span, which an editor shows.
    fn bytes(module: &Module, span: Span) -> Option<(String, u32, u32)> {
        if !span.is_known() {
            return None;
        }
        let file = module.files.get(span.file as usize)?.clone();
        Some((file, span.start, span.end))
    }

    fn place(module: &Module, span: Span) -> Option<(String, u32, u32)> {
        if !span.is_known() {
            return None;
        }
        let file = module.files.get(span.file as usize)?.clone();
        Some((file, span.line, span.column))
    }

    /// A failure of the module the definition lowered.
    #[must_use]
    pub fn validation(&self, module: &Module, error: &ValidationError) -> Problem {
        let at = Self::place(module, error.span);
        let span = Self::bytes(module, error.span);
        let Some(key) = error.key else {
            return Problem {
                warning: false,
                code: "KLF-SHD-9002".to_owned(),
                message: format!(
                    "definition `{}` lowered a module that breaks {}: {}",
                    self.lang, error.rule, error.message
                ),
                at,
                span,
                target: None,
            };
        };
        let code = self
            .keys
            .iter()
            .find(|(known, _)| known == key)
            .map(|(_, code)| *code);
        let Some(code) = code else {
            return Problem {
                warning: error.warning,
                code: "KLF-SHD-9003".to_owned(),
                message: format!(
                    "definition `{}` maps no code to `{key}`: {}",
                    self.lang, error.message
                ),
                at,
                span,
                target: None,
            };
        };
        let title = self
            .specs
            .iter()
            .find(|spec| spec.code == code)
            .map_or(key, |spec| spec.title.as_str());
        Problem {
            warning: error.warning,
            code: code.to_string(),
            message: format!("{title}: {}", error.message),
            at,
            span,
            target: None,
        }
    }
}
