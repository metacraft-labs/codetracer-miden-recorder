//! Recording logic for Miden VM execution traces.
//!
//! This module provides the top-level `record` function that reads a MASM
//! source file, runs it through the tracer, and writes CodeTracer output.

use std::path::Path;

use codetracer_trace_writer_nim::TraceEventsFileFormat;
use eyre::{Context, Result};

use crate::tracer::MidenTracer;

/// Record a Miden VM execution trace.
///
/// Reads the MASM source at `source_path`, assembles and executes it,
/// and writes CodeTracer trace files to `out_dir`.
pub fn record(source_path: &Path, out_dir: &Path, format: TraceEventsFileFormat) -> Result<()> {
    let source_code = std::fs::read_to_string(source_path)
        .with_context(|| format!("failed to read source file: {}", source_path.display()))?;

    MidenTracer::trace_program(source_path, &source_code, out_dir, format)
}
