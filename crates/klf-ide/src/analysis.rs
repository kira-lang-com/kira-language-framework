//! One analysis of a project at one revision (`KLF-EDITOR.md` section 8), and
//! the text form an analysis worker reads its request in and writes its
//! result out.

use klf_core::FileId;
use klf_diag::Severity;
use klf_facts::Facts;
use klf_facts::codec::{Records, unescape};
use std::path::PathBuf;

/// What to analyze: the driver arguments a `check` of the project takes, run
/// from `directory`, with the open documents standing in for their files.
/// `kir` asks for the KIR of every function as well (`KLF-EDITOR.md`
/// section 13.7), which only a project someone views KIR of pays for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalysisRequest {
    pub directory: PathBuf,
    pub arguments: Vec<String>,
    pub documents: Vec<(PathBuf, String)>,
    pub kir: bool,
}

/// One function's KIR text, raw as the unit lowered it and canonical after
/// the mandatory passes, with the span its definition named it at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KirFunction {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
    pub name: String,
    pub raw: String,
    pub canonical: String,
}

/// What a shader unit produced for one target or view (`KLF-EDITOR.md`
/// section 14.5). `kind` is `source`, `ksir`, `reflection` or `spirv`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderOutput {
    pub file: FileId,
    pub target: String,
    pub kind: String,
    /// The output file's name, which is what a lens and a view title show.
    pub name: String,
    pub language_id: String,
    pub text: String,
}

/// The mode a file of the analysis compiled in (`KLF-EDITOR.md` section
/// 13.5): the selected file mode's name, empty when none was selected, and
/// the effective memory model and convention as the definition spells them.
/// `unit` is the index of the unit holding the file, when one does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileInfo {
    pub file: FileId,
    pub mode: String,
    pub memory: String,
    pub convention: String,
    pub unit: Option<u32>,
}

/// What the macro `name`, invoked over `start..end` of `origin`, expanded
/// to (`KLF-EDITOR.md` section 13.6): the virtual file `file` and its text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expansion {
    pub origin: FileId,
    pub start: u32,
    pub end: u32,
    pub name: String,
    pub file: FileId,
    pub text: String,
}

/// A diagnostic of an analysis, at a byte span of one of its files.
/// `source` names the server the client shows it under; it differs from the
/// driver's name for a diagnostic one target of a shader refused
/// (`KLF-EDITOR.md` section 14.2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdeDiagnostic {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub source: Option<String>,
}

/// Why definition code stopped before an analysis finished (`KLF-EDITOR.md`
/// section 8.6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Interruption {
    /// The analysis was cancelled; its result is wanted no more.
    Cancelled,
    /// Definition code trapped: `code` names the trap and `query` what ran.
    Trapped { code: String, query: String },
}

/// What an analysis found: the files its session held, their diagnostics and
/// the facts the phases recorded. `failure` explains an analysis that stopped
/// early, and `interrupted` says when definition code was stopped; whatever
/// was recorded before is still here.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Analysis {
    pub files: Vec<(FileId, PathBuf)>,
    pub diagnostics: Vec<IdeDiagnostic>,
    pub facts: Facts,
    /// What each shader unit produced, in memory (section 14.5).
    pub shader_outputs: Vec<ShaderOutput>,
    pub file_info: Vec<FileInfo>,
    pub kir: Vec<KirFunction>,
    pub expansions: Vec<Expansion>,
    pub failure: Option<String>,
    pub interrupted: Option<Interruption>,
}

impl AnalysisRequest {
    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = Records::default();
        out.line(&["directory", &self.directory.to_string_lossy()]);
        for argument in &self.arguments {
            out.line(&["argument", argument]);
        }
        for (path, text) in &self.documents {
            out.line(&["document", &path.to_string_lossy(), text]);
        }
        if self.kir {
            out.line(&["kir"]);
        }
        out.text
    }

    /// # Errors
    /// Returns the first malformed line.
    pub fn decode(text: &str) -> Result<Self, String> {
        let mut request = Self::default();
        for line in text.lines() {
            let fields: Vec<String> = line.split('\t').map(unescape).collect();
            match fields.as_slice() {
                [key, directory] if key == "directory" => {
                    request.directory = PathBuf::from(directory);
                }
                [key, argument] if key == "argument" => request.arguments.push(argument.clone()),
                [key, path, text] if key == "document" => {
                    request.documents.push((PathBuf::from(path), text.clone()));
                }
                [key] if key == "kir" => request.kir = true,
                _ => return Err(format!("malformed analysis request line `{line}`")),
            }
        }
        Ok(request)
    }
}

impl Analysis {
    /// An analysis that produced nothing but its reason.
    #[must_use]
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            failure: Some(reason.into()),
            ..Self::default()
        }
    }

    /// An analysis whose definition code was stopped before it recorded
    /// anything.
    #[must_use]
    pub fn interrupted(interruption: Interruption) -> Self {
        Self {
            interrupted: Some(interruption),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn path(&self, file: FileId) -> Option<&PathBuf> {
        self.files
            .iter()
            .find(|(known, _)| *known == file)
            .map(|(_, path)| path)
    }

    #[must_use]
    pub fn file(&self, path: &std::path::Path) -> Option<FileId> {
        self.files
            .iter()
            .find(|(_, known)| known == path)
            .map(|(file, _)| *file)
    }

    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = Records::default();
        for (file, path) in &self.files {
            out.line(&["file", &file.as_u32().to_string(), &path.to_string_lossy()]);
        }
        for diagnostic in &self.diagnostics {
            out.line(&[
                "diagnostic",
                &diagnostic.file.as_u32().to_string(),
                &diagnostic.start.to_string(),
                &diagnostic.end.to_string(),
                severity_name(diagnostic.severity),
                &diagnostic.code,
                &diagnostic.message,
                diagnostic.source.as_deref().unwrap_or(""),
            ]);
        }
        if let Some(failure) = &self.failure {
            out.line(&["failure", failure]);
        }
        match &self.interrupted {
            Some(Interruption::Cancelled) => out.line(&["cancelled"]),
            Some(Interruption::Trapped { code, query }) => out.line(&["trapped", code, query]),
            None => {}
        }
        for output in &self.shader_outputs {
            out.line(&[
                "shader_output",
                &output.file.as_u32().to_string(),
                &output.target,
                &output.kind,
                &output.name,
                &output.language_id,
                &output.text,
            ]);
        }
        for info in &self.file_info {
            out.line(&[
                "file_info",
                &info.file.as_u32().to_string(),
                &info.mode,
                &info.memory,
                &info.convention,
                &info.unit.map_or_else(String::new, |unit| unit.to_string()),
            ]);
        }
        for function in &self.kir {
            out.line(&[
                "kir_function",
                &function.file.as_u32().to_string(),
                &function.start.to_string(),
                &function.end.to_string(),
                &function.name,
                &function.raw,
                &function.canonical,
            ]);
        }
        for expansion in &self.expansions {
            out.line(&[
                "expansion",
                &expansion.origin.as_u32().to_string(),
                &expansion.start.to_string(),
                &expansion.end.to_string(),
                &expansion.name,
                &expansion.file.as_u32().to_string(),
                &expansion.text,
            ]);
        }
        out.line(&["facts"]);
        out.text + &self.facts.encode()
    }

    /// # Errors
    /// Returns the first malformed line.
    pub fn decode(text: &str) -> Result<Self, String> {
        let mut analysis = Self::default();
        let mut lines = text.split_inclusive('\n');
        for line in lines.by_ref() {
            let line = line.trim_end_matches('\n');
            let fields: Vec<String> = line.split('\t').map(unescape).collect();
            match fields.as_slice() {
                [key] if key == "facts" => break,
                [key, file, target, kind, name, language_id, text] if key == "shader_output" => {
                    analysis.shader_outputs.push(ShaderOutput {
                        file: file_id(file)?,
                        target: target.clone(),
                        kind: kind.clone(),
                        name: name.clone(),
                        language_id: language_id.clone(),
                        text: text.clone(),
                    });
                }
                [key, file, mode, memory, convention, unit] if key == "file_info" => {
                    analysis.file_info.push(FileInfo {
                        file: file_id(file)?,
                        mode: mode.clone(),
                        memory: memory.clone(),
                        convention: convention.clone(),
                        unit: if unit.is_empty() {
                            None
                        } else {
                            Some(number(unit)?)
                        },
                    });
                }
                [key, file, start, end, name, raw, canonical] if key == "kir_function" => {
                    analysis.kir.push(KirFunction {
                        file: file_id(file)?,
                        start: number(start)?,
                        end: number(end)?,
                        name: name.clone(),
                        raw: raw.clone(),
                        canonical: canonical.clone(),
                    });
                }
                [key, origin, start, end, name, file, text] if key == "expansion" => {
                    analysis.expansions.push(Expansion {
                        origin: file_id(origin)?,
                        start: number(start)?,
                        end: number(end)?,
                        name: name.clone(),
                        file: file_id(file)?,
                        text: text.clone(),
                    });
                }
                [key, file, path] if key == "file" => {
                    analysis.files.push((file_id(file)?, PathBuf::from(path)));
                }
                [key, file, start, end, severity, code, message, source] if key == "diagnostic" => {
                    analysis.diagnostics.push(IdeDiagnostic {
                        file: file_id(file)?,
                        start: number(start)?,
                        end: number(end)?,
                        severity: severity_of(severity)
                            .ok_or_else(|| format!("unknown severity `{severity}`"))?,
                        code: code.clone(),
                        message: message.clone(),
                        source: (!source.is_empty()).then(|| source.clone()),
                    });
                }
                [key, failure] if key == "failure" => analysis.failure = Some(failure.clone()),
                [key] if key == "cancelled" => analysis.interrupted = Some(Interruption::Cancelled),
                [key, code, query] if key == "trapped" => {
                    analysis.interrupted = Some(Interruption::Trapped {
                        code: code.clone(),
                        query: query.clone(),
                    });
                }
                _ => return Err(format!("malformed analysis line `{line}`")),
            }
        }
        analysis.facts = Facts::decode(&lines.collect::<String>())?;
        Ok(analysis)
    }
}

fn number(field: &str) -> Result<u32, String> {
    field
        .parse()
        .map_err(|_| format!("`{field}` is not an offset"))
}

fn file_id(field: &str) -> Result<FileId, String> {
    number(field).map(FileId::from_u32)
}

#[must_use]
pub const fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
        Severity::Remark => "remark",
    }
}

#[must_use]
pub fn severity_of(name: &str) -> Option<Severity> {
    [
        Severity::Error,
        Severity::Warning,
        Severity::Note,
        Severity::Remark,
    ]
    .into_iter()
    .find(|severity| severity_name(*severity) == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_info_survives_the_worker_text() {
        let analysis = Analysis {
            files: vec![(FileId::from_u32(0), PathBuf::from("/p/a.mm"))],
            file_info: vec![
                FileInfo {
                    file: FileId::from_u32(0),
                    mode: "arc".to_owned(),
                    memory: "Counted".to_owned(),
                    convention: "Platform".to_owned(),
                    unit: Some(3),
                },
                FileInfo {
                    file: FileId::from_u32(1),
                    mode: String::new(),
                    memory: String::new(),
                    convention: String::new(),
                    unit: None,
                },
            ],
            ..Analysis::default()
        };
        assert_eq!(Analysis::decode(&analysis.encode()), Ok(analysis));
    }

    #[test]
    fn kir_survives_the_worker_text() {
        let request = AnalysisRequest {
            directory: PathBuf::from("/p"),
            kir: true,
            ..AnalysisRequest::default()
        };
        assert_eq!(AnalysisRequest::decode(&request.encode()), Ok(request));
        let analysis = Analysis {
            kir: vec![KirFunction {
                file: FileId::from_u32(0),
                start: 4,
                end: 8,
                name: "main".to_owned(),
                raw: "func module @main() -> i64 engine(any) {\nbb0():\n  return %r\n}\n"
                    .to_owned(),
                canonical: "func module @main() -> i64 engine(any) {\n}\n".to_owned(),
            }],
            expansions: vec![Expansion {
                origin: FileId::from_u32(0),
                start: 10,
                end: 15,
                name: "Printable".to_owned(),
                file: FileId::from_u32(7),
                text: "extend Point {\n}\n".to_owned(),
            }],
            ..Analysis::default()
        };
        assert_eq!(Analysis::decode(&analysis.encode()), Ok(analysis));
    }
}
