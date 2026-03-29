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
use codetracer_trace_writer::TraceEventsFileFormat;
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
    /// Path to the MASM source file (.masm) or pre-compiled package (.masp).
    program: PathBuf,

    /// Directory where the trace files will be written.
    ///
    /// The directory will be created if it does not exist.
    #[arg(short = 'o', long, default_value = "./ct-traces/")]
    out_dir: PathBuf,

    /// Output format for the trace data.
    #[arg(short = 'f', long, default_value = "binary")]
    format: OutputFormat,

    /// Treat the input as a pre-compiled .masp package (midenc output).
    ///
    /// A .masp file contains a MastForest with embedded DebugInfo produced
    /// by `cargo miden build`. This flag is currently a placeholder —
    /// full .masp support requires the midenc toolchain.
    #[arg(long)]
    masp: bool,
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
    // Handle .masp flag early — not yet supported.
    if args.masp {
        return Err(eyre::eyre!(
            "Pre-compiled .masp support requires the midenc/cargo-miden toolchain, \
             which is not currently available.\n\
             To record a Rust Miden program:\n  \
             1. Install cargo-miden: cargo install cargo-miden\n  \
             2. Build: cargo miden build --release\n  \
             3. Record the resulting .masp file with this flag"
        ));
    }

    // 1. Validate the source file exists
    let source_path = args
        .program
        .canonicalize()
        .with_context(|| format!("source file not found: {}", args.program.display()))?;

    eprintln!("Source file: {}", source_path.display());

    let format = match args.format {
        OutputFormat::Binary => TraceEventsFileFormat::Binary,
        OutputFormat::Json => TraceEventsFileFormat::Json,
    };

    // 2. Create the output directory
    let out_dir = &args.out_dir;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    // 3. Run the recorder
    codetracer_miden_recorder::recorder::record(&source_path, out_dir, format)?;

    eprintln!("Trace files written to {}", out_dir.display());

    Ok(())
}
