//! CLI entry point for the CodeTracer Miden recorder.
//!
//! Supports the `record` subcommand which loads a MASM source file,
//! executes it through the Miden VM, captures the execution trace,
//! and writes a CodeTracer CTFS trace bundle.
//!
//! # Usage
//!
//! ```text
//! codetracer-miden-recorder record <masm-file> --out-dir <output-dir>
//! ```
//!
//! The recorder always writes traces in the canonical CodeTracer multi-stream
//! CTFS format (see `Recorder-CLI-Conventions.md` §4 in `codetracer-specs`).
//! No `--format` flag is exposed: human-readable conversion is handled
//! out-of-band by `ct print` (shipped with `codetracer-trace-format-nim`).
//!
//! # Environment variables
//!
//! * `CODETRACER_MIDEN_RECORDER_OUT_DIR` — fallback for `--out-dir` when the
//!   flag is not given. The CLI flag always wins.
//! * `CODETRACER_MIDEN_RECORDER_DISABLED` — set to `1` or `true` to skip
//!   recording entirely. The recorder still validates its inputs (where
//!   applicable) and propagates a clean exit code.
//! * `CODETRACER_MIDEN_RECORDER_LOG_LEVEL` — recorder log verbosity (advisory;
//!   the Miden recorder currently logs to stderr unconditionally).

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use eyre::{Context, Result};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Environment variable used as a fallback for `--out-dir` when the CLI
/// flag is omitted.  Convention: see `Recorder-CLI-Conventions.md` §5.
const ENV_OUT_DIR: &str = "CODETRACER_MIDEN_RECORDER_OUT_DIR";

/// Environment variable that, when set to `1`/`true`, disables tracing
/// entirely — the recorder runs as a transparent pass-through.
const ENV_DISABLED: &str = "CODETRACER_MIDEN_RECORDER_DISABLED";

/// Default output directory used when neither `--out-dir` nor
/// `CODETRACER_MIDEN_RECORDER_OUT_DIR` is set.
const DEFAULT_OUT_DIR: &str = "./ct-traces/";

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// CodeTracer Miden recorder -- record Miden VM program execution traces.
///
/// Traces are always written in the canonical CTFS multi-stream format.
/// To convert a recorded `.ct` bundle to JSON / text for inspection, use
/// `ct print` from `codetracer-trace-format-nim`.
#[derive(Debug, Parser)]
#[command(
    name = "codetracer-miden-recorder",
    version,
    about = "Record Miden VM program execution traces for CodeTracer (CTFS-only). \
             Use `ct print` from codetracer-trace-format-nim for human-readable conversion.",
    long_about = "Record Miden VM program execution traces for CodeTracer.\n\
                  \n\
                  Output is always written in the canonical CodeTracer CTFS\n\
                  multi-stream format. Use `ct print` (shipped with the\n\
                  codetracer-trace-format-nim sibling) to convert a recorded\n\
                  `.ct` bundle to JSON or other human-readable forms.\n\
                  \n\
                  Environment variables:\n\
                    CODETRACER_MIDEN_RECORDER_OUT_DIR    fallback for --out-dir\n\
                    CODETRACER_MIDEN_RECORDER_DISABLED   set to 1/true to skip recording\n\
                    CODETRACER_MIDEN_RECORDER_LOG_LEVEL  log verbosity (advisory)"
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
    /// captures the execution trace, and writes a CTFS bundle to `--out-dir`.
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

#[derive(Debug, clap::Args)]
struct ContractArgs {
    /// Directory where the trace files will be written.
    ///
    /// Falls back to the `CODETRACER_MIDEN_RECORDER_OUT_DIR` environment
    /// variable when the flag is omitted.
    #[arg(short = 'o', long)]
    out_dir: Option<PathBuf>,

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
    /// The directory will be created if it does not exist.  Falls back to
    /// the `CODETRACER_MIDEN_RECORDER_OUT_DIR` environment variable when the
    /// flag is omitted.
    #[arg(short = 'o', long)]
    out_dir: Option<PathBuf>,

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
    ///
    /// Falls back to the `CODETRACER_MIDEN_RECORDER_OUT_DIR` environment
    /// variable when the flag is omitted.
    #[arg(short = 'o', long)]
    out_dir: Option<PathBuf>,

    /// Path to captured TransactionInputs JSON file.
    ///
    /// If provided, the replay uses these pre-captured inputs instead
    /// of syncing state from the node. This is the recommended approach
    /// for reliable replay.
    #[arg(long)]
    captured_inputs: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the effective output directory:
///   1. `--out-dir` if given on the CLI.
///   2. `CODETRACER_MIDEN_RECORDER_OUT_DIR` env var.
///   3. `DEFAULT_OUT_DIR` ("./ct-traces/").
fn resolve_out_dir(cli_out_dir: Option<PathBuf>) -> PathBuf {
    if let Some(path) = cli_out_dir {
        return path;
    }
    if let Some(value) = std::env::var_os(ENV_OUT_DIR)
        && !value.is_empty()
    {
        return PathBuf::from(value);
    }
    PathBuf::from(DEFAULT_OUT_DIR)
}

/// Whether the recorder is disabled via env var.  When true, the CLI
/// must execute its target operation in pass-through mode without
/// emitting any trace artefacts.
fn recording_disabled() -> bool {
    match std::env::var(ENV_DISABLED) {
        Ok(value) => {
            let v = value.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        }
        Err(_) => false,
    }
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

    if recording_disabled() {
        // Pass-through: the Miden recorder doesn't run a separate target
        // process — it assembles & executes the source itself — so disabling
        // recording simply means "don't emit any trace artefacts".
        eprintln!("{ENV_DISABLED} is set; skipping trace recording (no output written).");
        return Ok(());
    }

    // 2. Resolve and create the output directory
    let out_dir = resolve_out_dir(args.out_dir);
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    // 3. Run the recorder (CTFS only)
    codetracer_miden_recorder::recorder::record(&source_path, &out_dir)?;

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

    if recording_disabled() {
        eprintln!("{ENV_DISABLED} is set; skipping contract trace (no output written).");
        return Ok(());
    }

    let out_dir = resolve_out_dir(args.out_dir);

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
    let mut session = ContractTraceSession::new(config, out_dir.clone());
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

    if recording_disabled() {
        eprintln!("{ENV_DISABLED} is set; skipping replay recording (no output written).");
        return Ok(());
    }

    let out_dir = resolve_out_dir(args.out_dir);

    let mut config = client_replay::ReplayConfig::new(
        &args.node_url,
        &args.account_id,
        &args.transaction_id,
        &out_dir,
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
