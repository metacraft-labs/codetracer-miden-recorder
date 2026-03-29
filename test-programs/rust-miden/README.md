# Rust-via-midenc Test Program

This is a canonical Rust program for testing source-level debugging of
Miden programs compiled from Rust via the `midenc` compiler.

## Building

Requires `cargo-miden` (not available in the Nix dev shell):

```sh
cargo install cargo-miden
cargo miden build --release
```

The output `.masp` file contains a `MastForest` with embedded `DebugInfo`
that maps assembly operations back to source lines in `src/lib.rs`.

## Expected behavior

When traced, the program should produce debug events referencing
`src/lib.rs` line numbers instead of `.masm` lines. The tracer detects
this via the `Location.path` field ending in `.rs`.
