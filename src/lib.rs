//! CodeTracer recorder for Miden VM programs.
//!
//! This crate captures execution traces from the Miden VM and converts them
//! into the CodeTracer trace format for debugging and analysis.

pub mod client_replay;
pub mod kernel_procs;
pub mod mockchain;
pub mod recorder;
pub mod rust_support;
pub mod source_map;
pub mod tracer;
