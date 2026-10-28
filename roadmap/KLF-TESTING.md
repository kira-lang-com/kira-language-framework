# KLF Testing Specification v1.0

Date: 2026-10-05. Status: normative draft 1.0. Audience: anyone writing tests for a definition package, or working on `klf test`.

Companion to [`KLF-RIDL.md`](KLF-RIDL.md) section 31.1, which it extends: a corpus checks a built toolchain from the outside, and the `Testing` module checks a definition from the inside, on the VM, without building anything native. MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

Contents

1. Scope
2. Test files
3. `@Test` and `expect!`
4. The language under test
5. Running
6. Services
7. Decisions

---

## 1. Scope

Behaviour that belongs to a language, such as what a program returns or which diagnostic a phase reports, is tested in the language's own package, in KLF Kira. The kernel's Rust tests cover what only Rust can see: KIR verification, RIDL layout, backend parity and the driver's own surface. A Rust test that builds a release distribution is `#[ignore]`d and runs with `cargo nextest run --run-ignored all`.

A unit test compiles with the definition, so it reaches the definition's helpers directly. It runs on the VM, and the package loads once for the whole run.

## 2. Test files

Tests live in `<package>/tests/*.klfd`. A normal build of the package MUST NOT include them: `tests/` directly under the package root is excluded the way `std/` and `corpus/` are. A test file starts with `import Testing`, which gives the unit the module's declarations (section 4). `import Testing` is the only import besides `KLF` that KLF Kira accepts.

```
import Testing

@Test
function forLoopsCount() {
    expect!(run("int main() { int s = 0; for (int i = 0; i < 5; i = i + 1) { s = s + i; } return s; }") == 10)
}
```

String literals MAY use `"""` to span lines. The text is the lines between the delimiters' own lines, each without the closing delimiter's indentation:

```
    expect!(run("""
        int main() {
            return 42;
        }
        """) == 42)
```

## 3. `@Test` and `expect!`

`@Test` marks a function as a test. A test takes no parameters and returns nothing (`KLFK-E0207` otherwise), and `@Test` without `import Testing` is `KLFK-E0207` too. Each test runs on a fresh VM machine, in file order, then declaration order.

`expect!(condition)` evaluates a `Bool`. When it is false, the running test records an issue with the condition's source text and its `file:line:column`, and goes on. `name!(...)` is the macro call syntax: the `!` is written right after the name, with no space. `expect!` is the only macro so far, and it needs `import Testing`.

A test fails when it records an issue, traps, or fails. A trap ends the test and is its last issue.

## 4. The language under test

The module gives a test the language the package defines:

| Declaration | Meaning |
| --- | --- |
| `run(source) -> Int` | Compiles `source` as `input.<extension>`, with the language's first extension, and runs `main` on the VM. The result is the word `main` returns. A rejected program or a trap records an issue and gives 0. |
| `runFiles(sources: [TestSource]) -> Int` | The same for several files. `TestSource { name, text }` names each one, and an empty name is `input<n>.<extension>`. |
| `runPath(path) -> Int`, `runPaths(paths: [String]) -> Int` | The same for files of the package, read relative to its root under their own names. |
| `compiles(source) -> Bool`, `compilesFiles(sources) -> Bool` | Compiles only. The answer is whether the language accepted the program. |
| `formats(source, expected) -> Bool` | The language's `format` hook turns `source` into `expected`, and leaves `expected` as it is. A difference records an issue with the output. |
| `diagnosed(code, line, column) -> Bool` | The last compilation reported `error[code]` at `line:column`. A column of 0 matches any column. A phase's own diagnostics count even when the kernel wraps them in its phase failure. |
| `reported(text) -> Bool` | One of the last compilation's diagnostics, rendered as `<code>: <message>`, contains `text`. |

## 5. Running

`klf test <package>` runs the unit tests first, then the corpora of section 31.1, which build the toolchain. `--unit` runs only the unit tests, and `--corpus <name>` only that manifest. A package without `tests/` has no unit tests, and one without corpus manifests builds nothing. Each test prints `PASS <file>::<test>` or `FAIL <file>::<test>` with its issues.

## 6. Services

The module is KLF Kira over these kernel services, which only `klf test` serves. Elsewhere they answer with the error sentinel.

| Service | Signature |
| --- | --- |
| `klf.test.expect` | passed, condition text, location -> records an issue when not passed |
| `klf.test.source` | name, text -> adds a file to the next compilation |
| `klf.test.source_path` | path -> adds the package's file at `path`, 1 when read |
| `klf.test.run` | compiles the added files and runs `main` -> its result word |
| `klf.test.compile` | compiles the added files -> 1 when accepted |
| `klf.test.formats` | source, expected text -> 1 when formatted to it and stable |
| `klf.test.diagnosed` | code, line, column -> 1 when reported there |
| `klf.test.reported` | text -> 1 when a diagnostic contains it |

## 7. Decisions

- DT-001. Tests compile into the package's image instead of a separate program, so a test can call any helper the definition declares. The price is that a test file cannot reuse a name the definition already uses.
- DT-002. A failed `expect!` does not stop the test. One run reports every broken expectation, as Swift Testing's `#expect` does.
- DT-003. Results cross back as integers and booleans, never as strings, so no service has to allocate in the definition's heap. `diagnosed` and `reported` ask the question instead of handing over the text.
- DT-004. Tests run the program under test on the VM only. Backend parity is the kernel's concern and its Rust tests cover it.
