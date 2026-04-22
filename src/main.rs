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
use codetracer_trace_writer_nim::TraceEventsFileFormat;
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

    /// Record execution of a Miden contract in a MockChain environment.
    ///
    /// Uses MockChain from miden-testing to simulate a local blockchain with
    /// accounts, notes, and assets, then executes a transaction with tracing.
    ///
    /// NOTE: This subcommand currently requires miden-testing at a version
    /// compatible with our miden-processor dependency. Until versions are
    /// aligned, this operates on a simulated MockChain with synthetic data.
    Contract(ContractArgs),

    /// Replay an on-chain Miden transaction with tracing.
    ///
    /// Syncs state from a Miden node (or uses captured TransactionInputs)
    /// and re-executes a transaction with CodeTracer instrumentation to
    /// produce a full execution trace.
    ///
    /// NOTE: This subcommand currently requires either captured TransactionInputs
    /// or a compatible miden-client version. See `src/client_replay.rs` for
    /// known limitations of historical transaction replay.
    Replay(ReplayArgs),

    /// Print version information.
    Version,
}

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    Binary,
    Json,
}

#[derive(Debug, clap::Args)]
struct ContractArgs {
    /// Directory where the trace files will be written.
    #[arg(short = 'o', long, default_value = "./ct-traces/")]
    out_dir: PathBuf,

    /// Account ID for the wallet (hex, e.g. "0x1234").
    #[arg(long, default_value = "0x1000")]
    wallet_id: String,

    /// Account ID for the faucet (hex, e.g. "0x5678").
    #[arg(long, default_value = "0x2000")]
    faucet_id: String,

    /// Faucet token symbol.
    #[arg(long, default_value = "TEST")]
    symbol: String,

    /// Enable debug mode for transaction execution (tx_context_debug).
    #[arg(long, default_value_t = true)]
    debug: bool,
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

#[derive(Debug, clap::Args)]
struct ReplayArgs {
    /// URL of the Miden node's RPC endpoint.
    #[arg(long)]
    node_url: String,

    /// Account ID involved in the transaction (hex string, e.g. "0x1234").
    #[arg(long)]
    account_id: String,

    /// Transaction ID to replay (hex string).
    #[arg(long)]
    transaction_id: String,

    /// Directory where the trace files will be written.
    #[arg(short = 'o', long, default_value = "./ct-traces/")]
    out_dir: PathBuf,

    /// Path to captured TransactionInputs JSON file.
    ///
    /// If provided, the replay uses these pre-captured inputs instead
    /// of syncing state from the node. This is the recommended approach
    /// for reliable replay.
    #[arg(long)]
    captured_inputs: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Record(args) => record(args),
        Commands::Contract(args) => contract(args),
        Commands::Replay(args) => replay(args),
        Commands::Version => {
            println!("codetracer-miden-recorder {}", env!("CARGO_PKG_VERSION"));
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

// ---------------------------------------------------------------------------
// `contract` implementation
// ---------------------------------------------------------------------------

/// Execute the `contract` subcommand.
fn contract(args: ContractArgs) -> Result<()> {
    use codetracer_miden_recorder::mockchain::*;

    let wallet_id = parse_hex_id(&args.wallet_id)
        .with_context(|| format!("invalid wallet ID: {}", args.wallet_id))?;
    let faucet_id = parse_hex_id(&args.faucet_id)
        .with_context(|| format!("invalid faucet ID: {}", args.faucet_id))?;

    eprintln!(
        "Contract trace: wallet={}, faucet={}, symbol={}, debug={}",
        args.wallet_id, args.faucet_id, args.symbol, args.debug
    );

    // Build a MockChain configuration.
    let config = MockChainConfig {
        wallets: vec![WalletConfig {
            id: AccountId(wallet_id),
            initial_assets: AssetVault::default(),
        }],
        faucets: vec![FaucetConfig {
            id: AccountId(faucet_id),
            symbol: args.symbol.clone(),
            max_supply: 1_000_000,
            initial_supply: 0,
        }],
        p2id_notes: vec![P2IdNoteConfig {
            id: NoteId(1),
            sender: AccountId(faucet_id),
            receiver: AccountId(wallet_id),
            assets: AssetVault {
                fungible: vec![FungibleAsset {
                    faucet_id: AccountId(faucet_id),
                    amount: 100,
                }],
            },
            note_type: NoteType::Public,
            aux: 0,
        }],
        ..Default::default()
    };

    // Create and run the session.
    let mut session = ContractTraceSession::new(config, args.out_dir.clone());
    session.build_chain().map_err(|e| eyre::eyre!(e))?;

    let tx_config = TransactionConfig {
        account_id: AccountId(wallet_id),
        input_notes: vec![NoteId(1)],
        tx_script: None,
        debug_mode: args.debug,
    };
    session
        .execute_transaction(&tx_config)
        .map_err(|e| eyre::eyre!(e))?;

    let summary_path = session.finalize().map_err(|e| eyre::eyre!(e))?;
    eprintln!(
        "Contract trace summary written to {}",
        summary_path.display()
    );

    eprintln!(
        "NOTE: This is a simulated MockChain trace. Real contract tracing requires \
         miden-testing at a version compatible with miden-processor 0.13.x. \
         See src/mockchain.rs for migration instructions."
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// `replay` implementation
// ---------------------------------------------------------------------------

/// Execute the `replay` subcommand.
fn replay(args: ReplayArgs) -> Result<()> {
    use codetracer_miden_recorder::client_replay;

    eprintln!(
        "Replay: node={}, account={}, tx={}",
        args.node_url, args.account_id, args.transaction_id
    );

    let mut config = client_replay::ReplayConfig::new(
        &args.node_url,
        &args.account_id,
        &args.transaction_id,
        &args.out_dir,
    );

    if let Some(ref captured_path) = args.captured_inputs {
        config = config.with_captured_inputs(captured_path);
        eprintln!("Using captured inputs from: {}", captured_path.display());
    }

    match client_replay::replay_transaction(&config) {
        Ok(result) => {
            eprintln!(
                "Replay completed: block={}, notes={}, output={}",
                result.block_num,
                result.input_note_count,
                result.output_dir.display()
            );
            Ok(())
        }
        Err(e) => Err(eyre::eyre!("{e}")),
    }
}

/// Parse a hex string like "0x1000" or "1000" into a u64.
fn parse_hex_id(s: &str) -> Result<u64> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    u64::from_str_radix(s, 16).with_context(|| format!("invalid hex: {s}"))
}
