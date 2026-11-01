# 26.10.4

**Project:** Kira Language Framework (KLF)  
**Release week:** 2026-10-26 – 2026-11-01

## Highlights

Improved raw-memory lowering correctness for kernel-oriented development.

## Changed

- LLVM emission now preserves requested volatile and alignment flags on unmanaged raw loads and stores.
- Cranelift rejects unsupported flagged raw-memory operations instead of silently ignoring their semantics.
- Updated the implemented-features documentation to reflect the backend behavior.

## Validation

- `klf-llvm` and `klf-cranelift` compiled successfully; volatile/alignment LLVM regression passed.

## Known limitations

- Cranelift flagged-memory support and cross-backend parity are not yet implemented.
