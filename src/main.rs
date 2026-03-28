//! CLI entry point for the CodeTracer Miden recorder.
//!
//! Supports the `record` subcommand which loads a MASM source file,
//! executes it through the Miden VM, captures the execution trace,
//! and writes CodeTracer trace output files.
//!
//! # Usage
//!
//! ```text
//! codetracer-miden-recorder record <masm-file> \
//!     --out-dir <output-dir> \
//!     [--format binary|json]
//! ```

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use eyre::{Context, Result};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// CodeTracer Miden recorder — record Miden VM execution traces.
#[derive(Debug, Parser)]
#[command(
    name = "codetracer-miden-recorder",
    version,
    about = "Record Miden VM program execution traces for CodeTracer"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Record execution of a MASM program.
    ///
    /// Assembles and executes the given MASM source file through the Miden VM,
    /// captures the execution trace, and writes CodeTracer trace files to
    /// `--out-dir`.
    Record(RecordArgs),

    /// Print version information.
    Version,
}

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    Binary,
    Json,
}

#[derive(Debug, clap::Args)]
struct RecordArgs {
    /// Path to the MASM source file (.masm).
    program: PathBuf,

    /// Directory where the trace files will be written.
    ///
    /// The directory will be created if it does not exist.
    #[arg(short = 'o', long, default_value = "./ct-traces/")]
    out_dir: PathBuf,

    /// Output format for the trace data.
    #[arg(short = 'f', long, default_value = "binary")]
    format: OutputFormat,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Record(args) => record(args),
        Commands::Version => {
            println!(
                "codetracer-miden-recorder {}",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// `record` implementation
// ---------------------------------------------------------------------------

/// Execute the `record` subcommand.
fn record(args: RecordArgs) -> Result<()> {
    // 1. Validate the source file exists
    let source_path = args
        .program
        .canonicalize()
        .with_context(|| format!("source file not found: {}", args.program.display()))?;

    eprintln!("Source file: {}", source_path.display());

    // 2. Print not-yet-implemented message
    eprintln!("Recording is not yet implemented — writing placeholder trace files.");

    // 3. Create the output directory
    let out_dir = &args.out_dir;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    // 4. Write placeholder trace_metadata.json
    let metadata = serde_json::json!({
        "version": "0.1.0",
        "recorder": "codetracer-miden-recorder",
        "format": match args.format {
            OutputFormat::Binary => "binary",
            OutputFormat::Json => "json",
        },
        "source_file": source_path.to_string_lossy(),
        "status": "placeholder"
    });
    let metadata_path = out_dir.join("trace_metadata.json");
    std::fs::write(
        &metadata_path,
        serde_json::to_string_pretty(&metadata).unwrap(),
    )
    .with_context(|| format!("failed to write {}", metadata_path.display()))?;

    // 5. Write placeholder trace_paths.json
    let paths = serde_json::json!({
        "trace_metadata": "trace_metadata.json",
        "source_files": [source_path.to_string_lossy()]
    });
    let paths_path = out_dir.join("trace_paths.json");
    std::fs::write(
        &paths_path,
        serde_json::to_string_pretty(&paths).unwrap(),
    )
    .with_context(|| format!("failed to write {}", paths_path.display()))?;

    eprintln!("Trace files written to {}", out_dir.display());
    eprintln!("  trace_metadata.json");
    eprintln!("  trace_paths.json");

    // 6. Exit with code 0
    Ok(())
}
