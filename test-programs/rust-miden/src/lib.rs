// Canonical Rust test program for Miden VM debugging via midenc.
//
// This program performs a simple computation that exercises:
// - Variable assignments (mapped to local memory slots by midenc)
// - Arithmetic operations (mapped to stack operations)
// - Function calls (mapped to procedure calls)
//
// Expected values at each step:
//   a = 10
//   b = 32
//   sum = a + b = 42
//   doubled = sum + sum = 84
//   final_result = doubled + a = 94
//
// To build (requires cargo-miden):
//   cargo miden build --release
//
// The compiled .masp output will contain a MastForest with debug info
// that maps assembly operations back to line numbers in this file.

/// Helper function to double a value.
fn double(x: u32) -> u32 {
    x + x
}

/// Main computation entry point.
pub fn compute() -> u32 {
    let a: u32 = 10;
    let b: u32 = 32;
    let sum = a + b;
    let doubled = double(sum);
    let final_result = doubled + a;
    final_result
}
