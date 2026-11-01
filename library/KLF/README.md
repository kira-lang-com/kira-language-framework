# KLF for Kira frontends

`klf dist` installs this package at `share/klf/KLF`. Add it as a Kira
package dependency and `import KLF` in frontend source files. The package
provides the generated `KLFKernel` RIDL declarations, KCore-compatible
service wrappers, language, shader and editor interfaces, and driver helpers.
The KLF driver supplies the installed package when building a Kira frontend.
Other Kira packages can select it with `--dependency KLF=<KLF_HOME>/share/klf/KLF`;
the package can be used from native and KLF VM programs. Services that need a
frontend phase retain their normal context requirements.
The package selects the RIDL adapter archive for the compilation target from
the targets included in the distribution. The host platform is always included,
even when `klf dist --targets` names only other targets, so frontend drivers and
host Kira programs can use the distribution.
Driver hooks can use `runCommand` and `commandOutput` for captured child
processes, `runAttached` for attached processes, and the `writeOutput` and
`writeError` helpers for the driver's streams.
`klf editor gen <frontend> --out <directory>` uses its declared `EditorSpec`
to generate editor packages.
The shared `layoutTokens` formatter accepts source token spans and
`LayoutRules`; it uses the kernel's formatting services to preserve comments
and verify that formatting does not change tokens.
The `editorSymbolText`, `editorTypeAt`, `editorValueAt`, `editorHint`,
`editorLink`, and `editorRunnable` helpers record text-bearing editor facts
through the same services as KCore definitions.

Kira frontend tests can also import `KLF`. `run`, `runFiles`, `runPath`,
and `runPaths` compile with the frontend under test and run on the KLF VM;
`compiles`, `compilesFiles`, `formats`, `diagnosed`, and `reported` check
compilation and diagnostics. `runNative` executes a native program. These
helpers are available under `klf test <frontend> --unit`, which supplies the
driver, test directory, and the frontend's first source extension.
