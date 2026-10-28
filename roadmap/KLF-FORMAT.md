# KLF Formatting Specification v1.0

Date: 2026-10-05. Status: normative draft 1.0. Audience: anyone writing a definition's formatter, or working on the kernel's formatting engine, `<driver> format`, or the language server's formatting requests.

Companion to [`KLF-EDITOR.md`](KLF-EDITOR.md), which this document amends: it replaces DE-012 ("no kernel formatter") and section 11.17, and fixes the `format` hook's signature. MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

Contents

1. Scope
2. Split of work
3. The `format` hook
4. Layout services
5. What the kernel does
6. Commands and editors
7. The shader languages' layout
8. Shader outs
9. Decisions
10. Status

---

## 1. Scope

Every language, program or shader, formats through one kernel engine. The definition decides the style. The kernel does the rest, so every language handles comments, blank lines, line width and safety the same way.

Formatting is a whitespace transformation: the formatted file has the same tokens and the same comments in the same order as the original. Rewrites that change tokens (adding a trailing comma, removing parentheses, reordering imports) are code actions (`KLF-EDITOR.md` section 11.12), not formatting.

## 2. Split of work

| Who | Does |
| --- | --- |
| Definition | Parses the file as for a build, then describes a layout of its tree: the tokens in order, with groups, indentation and line breaks between them (section 4) |
| Kernel | Matches each token against the source, places comments, keeps blank lines, fits groups to the width, prints, checks that only whitespace changed, and turns the result into edits (section 5) |

A definition's lexer and tree need not keep comments or whitespace. The kernel reads them from the source using the comment syntax declared in the definition's `EditorSpec` (`lineComments`, `blockComments`).

## 3. The `format` hook

```
construct Language<Tree: Cacheable, Program: Cacheable> {
    // ...
    function format(session: Session, unit: Unit, tree: Tree) -> Bool
}
```

`ShaderLanguage` gains the same member.

1. The kernel runs `units` over the file and `parse` for the file's unit, then calls `format` with the tree, in the same heap. `elaborate` and `lower` do not run. The kernel-owned `emit` flag reads `format`, so a `parse` that loads more than the file, such as its package and imports, can parse the file alone.
2. If `parse` reports an error, the file is not formatted and the hook is not called.
3. The hook describes the layout through the services of section 4 and returns `true`. It returns `false` to decline the file; nothing is changed.
4. The hook MUST describe every token of the file. A token can be skipped only by covering it with `format_verbatim`.
5. The hook MAY report diagnostics as any phase does. An error declines the file.
6. Without a `format` member the language has no formatter: `<driver> format` fails and the server does not advertise formatting.

The lowerer generates `__klf_format(session, unit, tree) -> Int` around the hook. Natively it is trampoline selector 7.

## 4. Layout services

All of them answer 0 outside a `format` call.

| KLF Kira | Service | Effect |
| --- | --- | --- |
| `format_text(text: String) -> Int` | `klf.format.text` | The next token is `text`. 1 when the source has `text` next, after whitespace and comments |
| `format_token(start: Int, end: Int) -> Int` | `klf.format.token` | The next token is the source's `start..end` (byte offsets). For tokens the tree has spans for: names, literals |
| `format_verbatim(start: Int, end: Int) -> Int` | `klf.format.verbatim` | Keep `start..end` exactly as written, whitespace and comments inside included. For regions the formatter does not handle: preprocessor lines, embedded languages, code after a parse recovery |
| `format_line(kind: Int) -> Int` | `klf.format.line` | A line break. Kind 0 (soft) prints as nothing when its group is flat, 1 (space) as a space, 2 (hard) always breaks and breaks every group around it. Kind 3 is a space that never breaks |
| `format_group() -> Int` | `klf.format.group` | Opens a group: printed flat when it fits on the line, otherwise every line break directly in it breaks |
| `format_indent() -> Int` | `klf.format.indent` | Opens an indentation: lines broken inside it are indented one more level |
| `format_end() -> Int` | `klf.format.end` | Closes the innermost group or indentation |

This is Wadler's layout algebra as Prettier uses it. Groups nest, and an outer group breaks before an inner one.

Example, a call `f(a, b)`:

```
format_token(nameStart, nameEnd)
format_text("(")
format_group()
format_indent()
format_line(0)
// for each argument: the argument, then format_text(",") and format_line(1) between arguments
format_end()
format_line(0)
format_end()
format_text(")")
```

Printed as `f(a, b)` when it fits, otherwise one argument per line, indented, with `)` on its own line.

## 5. What the kernel does

### 5.1 Matching tokens

The kernel keeps a cursor in the source. For each token it skips whitespace and comments, then requires the token's text to come next. The first token that does not match ends the layout with `KLF-LSP-9002`. At the end only whitespace and comments may remain after the cursor. So a layout cannot drop, add, reorder or change a token.

### 5.2 Comments

Each comment is placed by where it sits in the source:

1. **Trailing:** on the same line as the token before it. A line comment is printed after the last token of that line, before the next line break. A block comment is printed after the token, with one space before it. Trailing comments are taken when the hook asks for a line break, so they stay on their line and do not move into the next statement.
2. **Leading:** on a line of its own. Printed on its own line before the next token, at that token's indentation. A block comment followed by a token on the same line stays on that line.

A line comment forces every group around it to break. Comment text is copied with trailing whitespace removed. The inside of a multi-line block comment is not re-indented.

### 5.3 Blank lines

One or more blank lines before a token or a leading comment become exactly one blank line, if that token starts a line. Blank lines at the start and end of the file are removed. The file ends with one line break.

### 5.4 Printing

Groups are printed flat when they fit in the width left on the line, looking ahead to the next line break after the group. A line has no trailing whitespace. Indentation is `tabSize` spaces per level, or one tab per level when `insertSpaces` is false. The default width is 100.

### 5.5 Checks

A layout is also rejected with `KLF-LSP-9002` when printing would join two tokens the source separated (two word characters, or two symbol characters such as `-` `-`), or split a word the source had in one piece. Together with 5.1 this replaces the re-lexing check of `KLF-EDITOR.md` 11.17: the token sequence is verified as it is built.

### 5.6 Edits

The kernel diffs the original and the formatted text by their whitespace runs: one edit per run that changed, so an editor keeps the cursor and marks. When comments moved, so the texts do not line up character for character, it answers one edit covering the changed middle. Range formatting formats the whole file and keeps the edits that touch the range.

## 6. Commands and editors

```
<driver> format [paths] [--check] [--stdin NAME] [--width N] [--tab-size N] [--tabs]
```

1. Each file is formatted in a session of its own. Other driver flags apply as for a build, so file modes are selected the same way.
2. Files are rewritten in place. With `--check` nothing is written; the files that would change are listed and the exit status is 1.
3. `--stdin NAME` formats standard input as the file `NAME` and writes the result to standard output.
4. A file that fails is reported as `<driver>: error: <path>: <reason>`, and the status is 1.

The language server advertises `documentFormattingProvider` and `documentRangeFormattingProvider` when the definition has a hook. It formats the open buffer by running `<driver> format --stdin` in a separate process, as analyses run, so a trap in definition code does not stop the server. A failure answers `RequestFailed` with the reason. On-type formatting is not served (section 9).

`klf-driver` exposes the same computation on the VM as `LanguageDefinition::format_source`, and `klf format <definition-package-dir> [files] [--check] [--width N]` runs it, formatting standard input when no file is given. It is for working on a formatter without building its toolchain.

## 7. The shared token layout

`klf-kira/languages/format/layout.klfd` is one layout for C-like token streams, which the KSL, MSL and Kira hooks share through their packages' `include` files (Kira's names the directory beside it, `../format`). A hook lexes its file with its own lexer and passes the token spans and a `LayoutRules` value to `layoutTokens`. A hook may pass several lexer tokens the source writes as one piece as one span, as Kira does for `+=` and for a quote's `#{...}` holes; the span is then laid out as written.

1. Line breaks between statements are the source's. Blank lines are kept by the kernel (5.3).
2. `{` of a block ends its line, except that a closure's parameters, names and commas up to an `in` that ends their line, stay on the `{` line, as in `{ graphics in`. Its content is indented and `}` starts its own line. `;` outside parentheses ends a line. A line the source breaks after or before a binary operator or a comma is indented one more level.
3. Parentheses and initializer braces are groups: flat when they fit, otherwise one item per line. Commas in brackets and template arguments never break on their own; a bracketed list the source breaks after its `[` keeps the source's line breaks, one more level in, with `]` on its own line. Initializer items the source separates by line breaks instead of commas keep those breaks.
4. Spacing: one space after `,`, `:` and `;` in a `for`, around binary operators, and between `}` and a following word. No space inside brackets and parentheses, around `.` and `::`, after a prefix operator, or between a name and its call's `(`. The words in `spacedCalls` and `keywords` take a space before `(`.
5. `<` and `>` around template arguments take no spaces: a `<` after a name opens arguments when a matching `>` comes before anything that cannot be an argument. `*` and `&` are spaced as binary after `)`, `]` or a number; elsewhere they may be part of a declaration (`device float* p`), so the source's spacing stays.
6. A `case` or `default` label ends its line, and its statements are indented under it.
7. A `#` that starts a line opens a directive, kept as written up to the end of its line, including continued lines.
8. A `.` after an operand takes no spaces; elsewhere it starts an implicit member, as in `return .Some(x)`, and is spaced like an operand.
9. A block's statements break lines even inside parentheses.
10. A header ends with the line it is written on, together with the lines rule 2 continues it on: a condition wrapped after `&&` still opens its block. A bodyless declaration's name is not an open header: the `{` of the next declaration opens that declaration's block or literal, as its own.

Rules a language turns on:

| Rule | Effect |
| --- | --- |
| `literals` | `{` opens a struct literal, laid out as an initializer with a space inside the braces, when the braces are empty or start with `name =` or `name:` and close on their own line or hold a comma at their level, unless it ends a header (a word of `headers` up to its block's `{`, outside the header's parentheses) |
| `inlineBlocks` | A block the source writes on one line is a group: it stays on one line, `{ return x }`, when it fits, and otherwise breaks as any block |
| `semicolons` | `;` ends a statement, and rule 7 applies |
| `labels`, `spacedArrow`, `spacedCalls`, `keywords`, `ambiguous` | As in rules 4 to 6 |

Kira uses `literals`, `inlineBlocks`, a spaced `->` and `<`, `>` as ambiguous.

The layout does not use the tree. It only needs the token stream, so a language with a parser can adopt it in a few lines.

## 8. Shader outs

A shader out (`KLF-KSL.md` section 5.8) writes text lines today. The intent is for outs to describe a layout with the same services, so converted shaders come out formatted like hand-written ones. That requires a layout without a source, where tokens are synthesized. It is planned with the S10 outs and is not part of this version. Until then, a converted shader can be cleaned up by running the target language's `<driver> format` over it.

Removing the temporaries and extra parentheses converted shaders carry is a KSIR pass, not formatting (section 1).

## 9. Decisions

| Id | Decision | Reason |
| --- | --- | --- |
| DF-001 | One kernel engine; the definition describes a layout, not text (replaces DE-012) | Without it each language writes its own comment handling, width fitting, edit diffing and safety checks, and they drift. The style is still the definition's |
| DF-002 | Comments are recovered from the source by the kernel, not kept in the definition's tree | No frontend's lexer keeps comments, and none needs them to compile. The `EditorSpec` already declares the comment syntax for every language |
| DF-003 | The token check runs while the layout is built, by matching against the source | It needs no second lexer, catches the error at the token that caused it, and works for languages whose lexer the kernel cannot run alone |
| DF-004 | The hook gets the parsed tree instead of re-parsing | The tree has the spans the layout needs; `parse` already runs in the same heap, as for `elaborate` |
| DF-005 | Formatting never adds or removes tokens | Keeps formatting a pure whitespace change that is always safe to apply. Token rewrites are code actions |
| DF-006 | Editors format through `<driver> format --stdin` in a separate process | Same isolation as analyses (`KLF-EDITOR.md` 8.6). The driver is the only place definition code runs |
| DF-007 | The shader formatters keep the source's statement line breaks and lay out tokens, not trees | KSL ends statements at line breaks its lexer drops, so the breaks have to come from the source. A token layout handles every construct the parser accepts, and the two languages share it |
| DF-008 | Parameter lists wrap one per line with a normal indent, not aligned under `(` | The engine has no alignment, and an indent survives renaming the function |

## 10. Status

Done:

- Kernel engine: `crates/klf-format` (layout, comments, blank lines, printer, edit diffing, checks).
- `klf.format.*` on the VM and native paths; the `format` hook in the lowerer, with the trampoline entry.
- `LanguageDefinition::format_source` on the VM; `<driver> format` in native frontends; formatting and range formatting in the language server.
- KSL, MSL and Kira formatters on the shared layout of section 7.
- Proof: `crates/klf-format/src/tests.rs`; `crates/klf-driver/tests/format.rs`: a definition written in KLF Kira, and the KSL, MSL and Kira definitions, formatted on the VM, each result stable when formatted again.
- `.Format` editor corpus checks: `the_editor_corpus_runs_against_the_drivers_language_server` in `crates/klf-driver/tests/native_frontend.rs`.
- Native proof: `native_frontend_formats_as_the_vm_does` in `crates/klf-driver/tests/native_frontend.rs` builds a native frontend from the same KLF Kira definition (`tests/lists.klfd`) and gets the VM's results from `<driver> format --stdin`, in place, and under `--check`.

Not done:

- A formatter for KArc. KArc's one frontend covers `.c`, `.cpp` and `.krc`, so its formatter covers all three. It formats the tokens it can map back to the source after preprocessing, and keeps directives and macro-produced regions with `format_verbatim`.
- On-type formatting.
- Shader outs printing through the engine (section 8).
