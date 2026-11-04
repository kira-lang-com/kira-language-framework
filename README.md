# Kira Language Framework

KLF is the Rust kernel, intermediate representation, runtime, and toolchain
framework specified by [`roadmap/KLF-RIDL.md`](roadmap/KLF-RIDL.md).

KCore is the definition language used to build Kira; `.klfd` remains accepted
as a compatibility extension for `.kcore` definitions. The KLF driver also
supports language frontends written in Kira. Kira frontends use the installed
KLF library and RIDL kernel interface, while the KLF driver continues to run
the frontend and provide its services. KCore and Kira shader languages can be
linked into the same driver.

`klf dist --out DIR` installs the KLF Kira library and its RIDL interface as
part of a distribution. `KLF_HOME` selects that distribution, and `KLF_KIRA`
can select the Kira compiler used to build Kira frontends. The
[`examples/kira-klf-consumer`](examples/kira-klf-consumer) and
[`examples/kira-shader`](examples/kira-shader) packages demonstrate the
library and frontend integration.

Run the current conformance gates with:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

`klf setup` installs the newest released `klf` and the RIDL LLVM it links
with into `~/.klf`, or into `KLFD_HOME` when set; nothing about it needs Rust.
CI builds each host's release with:

```sh
cargo xtask release OUT VERSION
```

From this monorepo checkout, `cargo xtask bootstrap-kira` builds KLF, uses it
to bootstrap Kira and `knvm`, then installs the current KLF and Kira checkouts
on the `dev` channel and installs `knvm` with its compiler launchers. Artifacts
go under Cargo's target directory. `knvm sinstall` also configures the active
shell to put the installed tools on `PATH`.

## macOS SDK linker compatibility

The bundled LLVM linker is the default. When a newer macOS SDK introduces
TAPI targets the bundled `ld64.lld` cannot yet parse, set
`KLF_USE_SYSTEM_LINKER=1` while building a native Apple executable or a Kira
language driver. KLF then asks the bundled clang to use Apple's system linker
instead. Linux and other targets keep their usual linker.
