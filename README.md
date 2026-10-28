# Kira Language Framework

KLF is the Rust kernel, intermediate representation, runtime, and toolchain
framework specified by [`roadmap/KLF-RIDL.md`](roadmap/KLF-RIDL.md).

The implementation is built milestone by milestone. M0 covers the KIR data
model, textual representation, and raw verifier rules V1 through V3.

Run the current conformance gates with:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

`klf setup` installs the newest released `klf` and the RIDL LLVM it links
with into `~/.klf`, or brings an install up to date; nothing about it needs
Rust. CI builds each host's release with:

```sh
cargo xtask release OUT VERSION
```
