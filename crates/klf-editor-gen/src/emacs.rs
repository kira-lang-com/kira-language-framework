//! The Emacs major mode (`KLF-EDITOR.md` section 19.8 item 3), for Emacs 29
//! and later: `treesit` when the grammar is installed, font-lock from the
//! editor spec otherwise, and the server registered with `eglot`.

use crate::regex::translate;
use crate::treesitter::{Dialect, queries_for};
use crate::{File, Language};
use klf_facts::TokenClass;
use klf_facts::pattern::Engine;
use std::fmt::Write;

fn elisp_string(text: &str) -> String {
    let mut out = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn face(class: TokenClass) -> &'static str {
    match class {
        TokenClass::Comment => "font-lock-comment-face",
        TokenClass::DocComment => "font-lock-doc-face",
        TokenClass::String | TokenClass::Character => "font-lock-string-face",
        TokenClass::Boolean | TokenClass::Constant | TokenClass::EnumMember => {
            "font-lock-constant-face"
        }
        TokenClass::Number => "font-lock-number-face",
        TokenClass::Operator => "font-lock-operator-face",
        TokenClass::Builtin => "font-lock-builtin-face",
        TokenClass::Annotation => "font-lock-preprocessor-face",
        TokenClass::Function | TokenClass::Method | TokenClass::Macro => {
            "font-lock-function-name-face"
        }
        TokenClass::Type
        | TokenClass::Struct
        | TokenClass::Class
        | TokenClass::Enum
        | TokenClass::Interface
        | TokenClass::TypeParameter
        | TokenClass::Namespace => "font-lock-type-face",
        TokenClass::Variable | TokenClass::Parameter | TokenClass::Property => {
            "font-lock-variable-name-face"
        }
        _ => "font-lock-keyword-face",
    }
}

/// A tree-sitter capture as the Emacs face of the same meaning.
fn capture_face(capture: &str) -> &'static str {
    match capture {
        "@comment" => "font-lock-comment-face",
        "@comment.documentation" => "font-lock-doc-face",
        "@string" | "@character" => "font-lock-string-face",
        "@string.escape" => "font-lock-escape-face",
        "@number" => "font-lock-number-face",
        "@boolean" | "@constant" => "font-lock-constant-face",
        "@operator" => "font-lock-operator-face",
        "@attribute" | "@keyword.directive" => "font-lock-preprocessor-face",
        "@function" | "@function.method" | "@function.macro" => "font-lock-function-call-face",
        "@type" | "@type.parameter" | "@module" => "font-lock-type-face",
        "@type.builtin" => "font-lock-builtin-face",
        "@punctuation.bracket" => "font-lock-bracket-face",
        "@punctuation.delimiter" => "font-lock-delimiter-face",
        "@variable" | "@variable.parameter" | "@property" => "font-lock-variable-name-face",
        _ => "font-lock-keyword-face",
    }
}

/// The highlight query in Emacs form: faces as captures, and without
/// `#any-of?`, which `treesit` lacks.
fn treesit_query(language: &Language) -> Result<String, String> {
    let queries = queries_for(language, Dialect::Neovim)?;
    let highlights = queries
        .iter()
        .find(|(name, _)| *name == "highlights.scm")
        .map(|(_, text)| text.clone())
        .unwrap_or_default();
    let mut out = String::new();
    for line in highlights.lines().filter(|line| !line.contains("#any-of?")) {
        let mut converted = String::new();
        for word in line.split(' ') {
            let (capture, rest) = match word.find(')') {
                Some(at) if word.starts_with('@') => word.split_at(at),
                _ if word.starts_with('@') => (word, ""),
                _ => ("", word),
            };
            if !converted.is_empty() {
                converted.push(' ');
            }
            if !capture.is_empty() {
                converted.push('@');
                converted.push_str(capture_face(capture));
            }
            converted.push_str(rest);
        }
        out.push_str(&converted);
        out.push('\n');
    }
    Ok(out)
}

pub(crate) fn package(language: &Language) -> Result<File, String> {
    let spec = &language.spec;
    let id = language.identifier();
    let mut syntax = String::new();
    let line = spec.line_comments.first().map(String::as_str);
    let block = spec.block_comments.first();
    match (line, block) {
        (Some("//"), Some(block)) if block.open == "/*" && block.close == "*/" => {
            let nests = if block.nests { "n" } else { "" };
            let _ = writeln!(
                syntax,
                "    (modify-syntax-entry ?/ \". 124b\" table)\n    (modify-syntax-entry ?* \". 23{nests}\" table)\n    (modify-syntax-entry ?\\n \"> b\" table)"
            );
        }
        (Some(prefix), _) if prefix.chars().count() == 1 => {
            let _ = writeln!(
                syntax,
                "    (modify-syntax-entry ?{prefix} \"<\" table)\n    (modify-syntax-entry ?\\n \">\" table)"
            );
        }
        _ => {}
    }
    for form in &spec.strings {
        if form.open == form.close && form.open.chars().count() == 1 {
            let quote = &form.open;
            let _ = writeln!(
                syntax,
                "    (modify-syntax-entry ?{}{quote} \"\\\"\" table)",
                if quote == "\"" { "\\" } else { "" }
            );
        }
    }
    if spec.strings.iter().any(|form| {
        form.escape
            .as_deref()
            .is_some_and(|escape| escape.starts_with("\\\\"))
    }) {
        syntax.push_str("    (modify-syntax-entry ?\\\\ \"\\\\\" table)\n");
    }

    let mut keywords = String::new();
    for group in &spec.keywords {
        let words: Vec<String> = group.words.iter().map(|word| elisp_string(word)).collect();
        let _ = writeln!(
            keywords,
            "    (,(regexp-opt '({}) 'symbols) . {})",
            words.join(" "),
            face(group.class)
        );
    }
    if let Some(annotation) = &spec.annotation {
        let _ = writeln!(
            keywords,
            "    ({} . font-lock-preprocessor-face)",
            elisp_string(&translate("annotation", annotation, Engine::Emacs)?)
        );
    }
    for (index, number) in spec.numbers.iter().enumerate() {
        let _ = writeln!(
            keywords,
            "    ({} . font-lock-constant-face)",
            elisp_string(&format!(
                "\\_<{}",
                translate(&format!("numbers[{index}]"), number, Engine::Emacs)?
            ))
        );
    }
    let comment_start = line.map_or_else(
        || block.map_or_else(String::new, |block| format!("{} ", block.open)),
        |prefix| format!("{prefix} "),
    );
    let comment_end = if line.is_none() {
        block.map_or_else(String::new, |block| format!(" {}", block.close))
    } else {
        String::new()
    };
    let extensions: Vec<String> = language
        .extensions
        .iter()
        .map(|extension| elisp_string(extension))
        .collect();
    let text = TEMPLATE
        .replace("@ID@", &id)
        .replace("@NAME@", &spec.display_name)
        .replace("@DRIVER@", &elisp_string(&language.driver))
        .replace("@SYSROOT@", &elisp_string(&language.sysroot_variable()))
        .replace("@SYNTAX@", syntax.trim_end())
        .replace("@KEYWORDS@", keywords.trim_end())
        .replace("@QUERY@", &elisp_string(&treesit_query(language)?))
        .replace("@COMMENT_START@", &elisp_string(&comment_start))
        .replace("@COMMENT_END@", &elisp_string(&comment_end))
        .replace(
            "@WIDTH@",
            &spec.indent.unit.chars().count().max(1).to_string(),
        )
        .replace("@TABS@", if spec.indent.unit == "\t" { "t" } else { "nil" })
        .replace("@EXTENSIONS@", &extensions.join(" "));
    Ok(File::text(format!("emacs/{id}-mode.el"), text))
}

const TEMPLATE: &str = r#";;; @ID@-mode.el --- Major mode for @NAME@  -*- lexical-binding: t; -*-

;; Generated by klf editor gen.

;;; Commentary:

;; Highlights with treesit when the `@ID@' grammar is installed and with
;; font-lock otherwise, and registers the language server with eglot.

;;; Code:

(require 'eglot)
(require 'treesit nil t)

(defgroup @ID@ nil
  "@NAME@ support."
  :group 'languages)

(defcustom @ID@-server-path ""
  "Path of the @NAME@ language server; empty to search the sysroot and PATH."
  :type 'string
  :group '@ID@)

(defvar @ID@-mode-syntax-table
  (let ((table (make-syntax-table)))
@SYNTAX@
    table)
  "Syntax table of `@ID@-mode'.")

(defconst @ID@--font-lock-keywords
  `(
@KEYWORDS@
    )
  "Font-lock rules of `@ID@-mode', from the editor spec.")

(defun @ID@--server-command (&optional _interactive)
  "The language server command, found as KLF-EDITOR.md section 19.6 says."
  (let ((sysroot (getenv @SYSROOT@)))
    (cond
     ((not (string-empty-p @ID@-server-path)) (list @ID@-server-path "lsp"))
     ((and sysroot (not (string-empty-p sysroot)))
      (list (expand-file-name (concat "bin/" @DRIVER@) sysroot) "lsp"))
     ((executable-find @DRIVER@) (list (executable-find @DRIVER@) "lsp"))
     (t (user-error "@NAME@: no language server; set `@ID@-server-path', set %s, or put %s on PATH"
                    @SYSROOT@ @DRIVER@)))))

;;;###autoload
(define-derived-mode @ID@-mode prog-mode "@NAME@"
  "Major mode for @NAME@."
  :syntax-table @ID@-mode-syntax-table
  (setq-local comment-start @COMMENT_START@)
  (setq-local comment-end @COMMENT_END@)
  (setq-local tab-width @WIDTH@)
  (setq-local indent-tabs-mode @TABS@)
  (if (and (fboundp 'treesit-ready-p) (treesit-ready-p '@ID@ t))
      (progn
        (treesit-parser-create '@ID@)
        (setq-local treesit-font-lock-settings
                    (treesit-font-lock-rules :language '@ID@ :feature 'all @QUERY@))
        (setq-local treesit-font-lock-feature-list '((all)))
        (treesit-major-mode-setup))
    (setq-local font-lock-defaults '(@ID@--font-lock-keywords))))

;;;###autoload
(add-to-list 'auto-mode-alist
             (cons (concat "\\." (regexp-opt '(@EXTENSIONS@)) "\\'") '@ID@-mode))

(add-to-list 'eglot-server-programs '(@ID@-mode . @ID@--server-command))

(provide '@ID@-mode)

;;; @ID@-mode.el ends here
"#;
