//! Recording logic for Miden VM execution traces.
//!
//! This module provides the top-level `record` function that reads a MASM
//! source file, runs it through the tracer, and writes a CodeTracer CTFS
//! trace bundle.
//!
//! The output format is fixed to CTFS — see
//! `Recorder-CLI-Conventions.md` §4 in `codetracer-specs`.  Use
//! `ct print` (from `codetracer-trace-format-nim`) for human-readable
//! conversion of the produced bundle.

use std::path::Path;

use eyre::{Context, Result};

use crate::tracer::MidenTracer;

/// Record a Miden VM execution trace.
///
/// Reads the MASM source at `source_path`, assembles and executes it,
/// and writes a CTFS trace bundle to `out_dir`.
pub fn record(source_path: &Path, out_dir: &Path) -> Result<()> {
    let source_code = std::fs::read_to_string(source_path)
        .with_context(|| format!("failed to read source file: {}", source_path.display()))?;

    MidenTracer::trace_program(source_path, &source_code, out_dir)
}
