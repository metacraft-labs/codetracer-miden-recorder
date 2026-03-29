//! MockChain contract-level testing support for the Miden tracer.
//!
//! This module provides infrastructure for tracing Miden smart contract
//! execution within a MockChain test environment. MockChain (from miden-testing)
//! simulates a local blockchain with accounts, notes, assets, and block
//! production without running a real node.
//!
//! # Current Status
//!
//! The `miden-testing` crate is available on crates.io at version 0.14.0, but
//! this recorder currently depends on `miden-processor` 0.13.x. The version
//! mismatch means we cannot use `miden-testing` directly. This module provides
//! types and traits that mirror the expected MockChain API so that:
//!
//! 1. The architecture is established and tested with synthetic data
//! 2. When miden-* versions are aligned, swapping in the real crate is straightforward
//! 3. Tests validate the full lifecycle without requiring the actual MockChain
//!
//! # Architecture
//!
//! ```text
//! MockChainConfig       -- Describes the test chain (accounts, notes, assets)
//!   |
//!   v
//! MockChainBuilder      -- Constructs the chain from config
//!   |
//!   v
//! MockChainState        -- The built chain state (mirroring MockChain)
//!   |
//!   v
//! TransactionConfig     -- Describes a transaction to execute
//!   |
//!   v
//! ContractTraceSession  -- Manages: build chain -> construct tx -> execute -> trace
//!   |
//!   v
//! ExecutionContext       -- Tracks context switches within a transaction
//! ```
//!
//! # Integration with Real miden-testing
//!
//! When miden-testing becomes usable (version alignment with miden-processor),
//! the migration path is:
//!
//! 1. Add `miden-testing = { version = "X", features = ["tx_context_debug"] }` to Cargo.toml
//! 2. Replace `MockChainState` internals with real `MockChain`
//! 3. Replace `TransactionConfig::execute` with real `TransactionContext::execute`
//! 4. Keep `ContractTraceSession` and `ExecutionContext` as-is (they wrap our tracer)

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::kernel_procs;

// ---------------------------------------------------------------------------
// Account and Asset Types (mirroring miden-base types)
// ---------------------------------------------------------------------------

/// Unique identifier for an account in the MockChain.
///
/// In the real Miden protocol, `AccountId` is a 64-bit identifier derived
/// from the account's initial state commitment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId(pub u64);

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016x}", self.0)
    }
}

/// Unique identifier for a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteId(pub u64);

/// A fungible asset amount.
///
/// In Miden, fungible assets are represented as a `(faucet_id, amount)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FungibleAsset {
    /// The faucet that issued this asset.
    pub faucet_id: AccountId,
    /// The amount (in the faucet's smallest unit).
    pub amount: u64,
}

/// A collection of assets held by an account or note.
#[derive(Debug, Clone, Default)]
pub struct AssetVault {
    pub fungible: Vec<FungibleAsset>,
}

// ---------------------------------------------------------------------------
// Account Configuration
// ---------------------------------------------------------------------------

/// Configuration for a wallet account.
#[derive(Debug, Clone)]
pub struct WalletConfig {
    /// The account ID to assign (in real Miden this is derived; here it's explicit for testing).
    pub id: AccountId,
    /// Initial assets held by the wallet.
    pub initial_assets: AssetVault,
}

/// Configuration for a basic faucet account.
#[derive(Debug, Clone)]
pub struct FaucetConfig {
    /// The faucet account ID.
    pub id: AccountId,
    /// Token symbol (e.g., "TEST").
    pub symbol: String,
    /// Maximum supply the faucet can ever mint.
    pub max_supply: u64,
    /// Initial supply already minted.
    pub initial_supply: u64,
}

// ---------------------------------------------------------------------------
// Note Configuration
// ---------------------------------------------------------------------------

/// The type of a note (public, private, or encrypted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteType {
    Public,
    Private,
    Encrypted,
}

/// Configuration for a pay-to-id note.
#[derive(Debug, Clone)]
pub struct P2IdNoteConfig {
    /// The note ID.
    pub id: NoteId,
    /// Sender account.
    pub sender: AccountId,
    /// Receiver account.
    pub receiver: AccountId,
    /// Assets in the note.
    pub assets: AssetVault,
    /// Note type.
    pub note_type: NoteType,
    /// Auxiliary data.
    pub aux: u64,
}

/// Configuration for a swap note.
#[derive(Debug, Clone)]
pub struct SwapNoteConfig {
    /// The note ID.
    pub id: NoteId,
    /// Sender account.
    pub sender: AccountId,
    /// Assets offered by the sender.
    pub offered: AssetVault,
    /// Assets requested in return.
    pub requested: AssetVault,
    /// Note type.
    pub note_type: NoteType,
    /// Auxiliary data.
    pub aux: u64,
}

/// A generic output note.
#[derive(Debug, Clone)]
pub struct OutputNote {
    pub id: NoteId,
    pub sender: AccountId,
    pub assets: AssetVault,
    pub note_type: NoteType,
}

// ---------------------------------------------------------------------------
// MockChain Configuration and Builder
// ---------------------------------------------------------------------------

/// Full configuration for a MockChain instance.
///
/// This mirrors the builder pattern of `MockChainBuilder` from miden-testing:
/// ```ignore
/// MockChain::builder()
///     .add_existing_wallet(auth)
///     .add_existing_basic_faucet(auth, "TEST", 1_000_000, 0)
///     .add_p2id_note(sender, receiver, assets, NoteType::Public, 0)
///     .build()
/// ```
#[derive(Debug, Clone, Default)]
pub struct MockChainConfig {
    /// Wallet accounts to create.
    pub wallets: Vec<WalletConfig>,
    /// Faucet accounts to create.
    pub faucets: Vec<FaucetConfig>,
    /// Pay-to-ID notes to add.
    pub p2id_notes: Vec<P2IdNoteConfig>,
    /// Swap notes to add.
    pub swap_notes: Vec<SwapNoteConfig>,
    /// Generic output notes to add.
    pub output_notes: Vec<OutputNote>,
}

/// Builder for constructing a MockChain configuration.
///
/// Mirrors the `MockChainBuilder` API from miden-testing.
pub struct MockChainBuilder {
    config: MockChainConfig,
}

impl MockChainBuilder {
    /// Create a new builder with empty configuration.
    pub fn new() -> Self {
        Self {
            config: MockChainConfig::default(),
        }
    }

    /// Add a wallet account.
    ///
    /// Mirrors `MockChainBuilder::add_existing_wallet(auth)`.
    pub fn add_existing_wallet(mut self, wallet: WalletConfig) -> Self {
        self.config.wallets.push(wallet);
        self
    }

    /// Add a basic faucet account.
    ///
    /// Mirrors `MockChainBuilder::add_existing_basic_faucet(auth, symbol, max_supply, supply)`.
    pub fn add_existing_basic_faucet(mut self, faucet: FaucetConfig) -> Self {
        self.config.faucets.push(faucet);
        self
    }

    /// Add a pay-to-ID note.
    ///
    /// Mirrors `MockChainBuilder::add_p2id_note(sender, receiver, assets, note_type, aux)`.
    pub fn add_p2id_note(mut self, note: P2IdNoteConfig) -> Self {
        self.config.p2id_notes.push(note);
        self
    }

    /// Add a swap note.
    ///
    /// Mirrors `MockChainBuilder::add_swap_note(sender, offered, requested, note_type, aux)`.
    pub fn add_swap_note(mut self, note: SwapNoteConfig) -> Self {
        self.config.swap_notes.push(note);
        self
    }

    /// Add a generic output note.
    ///
    /// Mirrors `MockChainBuilder::add_output_note(note)`.
    pub fn add_output_note(mut self, note: OutputNote) -> Self {
        self.config.output_notes.push(note);
        self
    }

    /// Build the MockChain state from the configuration.
    ///
    /// Mirrors `MockChainBuilder::build() -> MockChain`.
    pub fn build(self) -> MockChainState {
        let mut accounts = BTreeMap::new();
        for w in &self.config.wallets {
            accounts.insert(
                w.id,
                AccountState {
                    id: w.id,
                    kind: AccountKind::Wallet,
                    assets: w.initial_assets.clone(),
                },
            );
        }
        for f in &self.config.faucets {
            accounts.insert(
                f.id,
                AccountState {
                    id: f.id,
                    kind: AccountKind::Faucet {
                        symbol: f.symbol.clone(),
                        max_supply: f.max_supply,
                        current_supply: f.initial_supply,
                    },
                    assets: AssetVault::default(),
                },
            );
        }

        let mut notes = BTreeMap::new();
        for n in &self.config.p2id_notes {
            notes.insert(
                n.id,
                NoteState {
                    id: n.id,
                    sender: n.sender,
                    assets: n.assets.clone(),
                    note_type: n.note_type,
                    consumed: false,
                },
            );
        }
        for n in &self.config.swap_notes {
            notes.insert(
                n.id,
                NoteState {
                    id: n.id,
                    sender: n.sender,
                    assets: n.offered.clone(),
                    note_type: n.note_type,
                    consumed: false,
                },
            );
        }
        for n in &self.config.output_notes {
            notes.insert(
                n.id,
                NoteState {
                    id: n.id,
                    sender: n.sender,
                    assets: n.assets.clone(),
                    note_type: n.note_type,
                    consumed: false,
                },
            );
        }

        MockChainState {
            config: self.config,
            accounts,
            notes,
            blocks: Vec::new(),
            block_number: 0,
        }
    }
}

impl Default for MockChainBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// MockChain State (mirrors MockChain struct)
// ---------------------------------------------------------------------------

/// The kind of a Miden account.
#[derive(Debug, Clone)]
pub enum AccountKind {
    Wallet,
    Faucet {
        symbol: String,
        max_supply: u64,
        current_supply: u64,
    },
}

/// State of an account in the MockChain.
#[derive(Debug, Clone)]
pub struct AccountState {
    pub id: AccountId,
    pub kind: AccountKind,
    pub assets: AssetVault,
}

/// State of a note in the MockChain.
#[derive(Debug, Clone)]
pub struct NoteState {
    pub id: NoteId,
    pub sender: AccountId,
    pub assets: AssetVault,
    pub note_type: NoteType,
    pub consumed: bool,
}

/// A block produced by the MockChain.
#[derive(Debug, Clone)]
pub struct Block {
    pub number: u64,
    pub transactions: Vec<TransactionRecord>,
}

/// Record of an executed transaction.
#[derive(Debug, Clone)]
pub struct TransactionRecord {
    /// The account that executed the transaction.
    pub account_id: AccountId,
    /// Notes consumed in this transaction.
    pub consumed_notes: Vec<NoteId>,
    /// Whether the transaction succeeded.
    pub success: bool,
}

/// The built MockChain state.
///
/// This mirrors the key fields of `MockChain` from miden-testing:
/// - `committed_accounts: BTreeMap<AccountId, Account>`
/// - `committed_notes: BTreeMap<NoteId, Note>`
/// - `blocks: Vec<Block>`
pub struct MockChainState {
    /// The original configuration used to build this chain.
    pub config: MockChainConfig,
    /// Accounts in the chain, keyed by account ID.
    pub accounts: BTreeMap<AccountId, AccountState>,
    /// Notes in the chain, keyed by note ID.
    pub notes: BTreeMap<NoteId, NoteState>,
    /// Produced blocks.
    pub blocks: Vec<Block>,
    /// Current block number.
    pub block_number: u64,
}

impl MockChainState {
    /// Get an account by ID.
    pub fn get_account(&self, id: &AccountId) -> Option<&AccountState> {
        self.accounts.get(id)
    }

    /// Get a note by ID.
    pub fn get_note(&self, id: &NoteId) -> Option<&NoteState> {
        self.notes.get(id)
    }

    /// Check if an account exists in the chain.
    pub fn has_account(&self, id: &AccountId) -> bool {
        self.accounts.contains_key(id)
    }

    /// Number of accounts in the chain.
    pub fn account_count(&self) -> usize {
        self.accounts.len()
    }

    /// Number of notes in the chain.
    pub fn note_count(&self) -> usize {
        self.notes.len()
    }

    /// Record a transaction execution.
    ///
    /// Mirrors `MockChain::add_pending_executed_transaction(&executed_tx)`.
    pub fn record_transaction(&mut self, record: TransactionRecord) {
        // Mark consumed notes.
        for note_id in &record.consumed_notes {
            if let Some(note) = self.notes.get_mut(note_id) {
                note.consumed = true;
            }
        }
        // Add to pending block (append to current block, or create one).
        if let Some(block) = self.blocks.last_mut() {
            block.transactions.push(record);
        } else {
            self.blocks.push(Block {
                number: self.block_number,
                transactions: vec![record],
            });
        }
    }

    /// Produce the next block.
    ///
    /// Mirrors `MockChain::prove_next_block()`.
    pub fn produce_block(&mut self) {
        self.block_number += 1;
    }
}

// ---------------------------------------------------------------------------
// Transaction Configuration
// ---------------------------------------------------------------------------

/// Configuration for a transaction to execute within the MockChain.
///
/// Mirrors the TransactionContext construction:
/// ```ignore
/// mock_chain.build_tx_context(TxContextInput::AccountId(account_id), &[note_id], &[])
///     .tx_script(script)
///     .authenticator(Some(auth))
///     .build()
/// ```
#[derive(Debug, Clone)]
pub struct TransactionConfig {
    /// The account executing the transaction.
    pub account_id: AccountId,
    /// Notes to consume in this transaction.
    pub input_notes: Vec<NoteId>,
    /// Optional transaction script (MASM source).
    pub tx_script: Option<String>,
    /// Whether debug mode is enabled (tx_context_debug feature).
    pub debug_mode: bool,
}

// ---------------------------------------------------------------------------
// Execution Context Tracking
// ---------------------------------------------------------------------------

/// Identifies an execution context within a transaction.
///
/// In Miden transactions, the VM switches between multiple execution contexts:
/// - Context 0: kernel/prologue context
/// - Context 1+: account code, note scripts, transaction script
///
/// Each context may execute different programs with different source files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContextId(pub u32);

impl fmt::Display for ContextId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ctx:{}", self.0)
    }
}

/// The kind of execution context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextKind {
    /// Kernel context — system calls, prologue/epilogue.
    Kernel,
    /// Account code execution.
    AccountCode { account_id: AccountId },
    /// Note script execution.
    NoteScript { note_id: NoteId },
    /// Transaction script execution.
    TxScript,
    /// Unknown context.
    Unknown,
}

/// Information about a single execution context.
#[derive(Debug, Clone)]
pub struct ExecutionContextInfo {
    /// The context ID assigned by the VM.
    pub id: ContextId,
    /// What kind of context this is.
    pub kind: ContextKind,
    /// Source file associated with this context (if known).
    pub source_path: Option<PathBuf>,
    /// Number of clock cycles spent in this context.
    pub cycle_count: u64,
    /// Whether this context has been entered at least once.
    pub entered: bool,
}

/// Tracks execution context switches during a transaction.
///
/// During transaction execution, the Miden VM switches between contexts
/// as it executes kernel procedures, account code, note scripts, and
/// transaction scripts. This tracker records all context switches and
/// associates each context with its kind and source.
pub struct ExecutionContextTracker {
    /// All contexts seen during execution, in order of first appearance.
    contexts: BTreeMap<ContextId, ExecutionContextInfo>,
    /// The currently active context.
    current_context: Option<ContextId>,
    /// History of context switches (context_id, clock_cycle).
    switch_history: Vec<(ContextId, u64)>,
}

impl ExecutionContextTracker {
    /// Create a new tracker.
    pub fn new() -> Self {
        Self {
            contexts: BTreeMap::new(),
            current_context: None,
            switch_history: Vec::new(),
        }
    }

    /// Record a context switch at the given clock cycle.
    ///
    /// If the context has not been seen before, it is registered with the
    /// given kind. If the context ID matches the current context, this is
    /// a no-op.
    pub fn switch_context(
        &mut self,
        context_id: ContextId,
        kind: ContextKind,
        clock_cycle: u64,
    ) {
        if self.current_context == Some(context_id) {
            // Already in this context; just increment the cycle count.
            if let Some(info) = self.contexts.get_mut(&context_id) {
                info.cycle_count += 1;
            }
            return;
        }

        self.current_context = Some(context_id);
        self.switch_history.push((context_id, clock_cycle));

        self.contexts
            .entry(context_id)
            .and_modify(|info| {
                info.cycle_count += 1;
                info.entered = true;
            })
            .or_insert(ExecutionContextInfo {
                id: context_id,
                kind,
                source_path: None,
                cycle_count: 1,
                entered: true,
            });
    }

    /// Set the source path for a context.
    pub fn set_source_path(&mut self, context_id: ContextId, path: PathBuf) {
        if let Some(info) = self.contexts.get_mut(&context_id) {
            info.source_path = Some(path);
        }
    }

    /// Get the current context ID.
    pub fn current(&self) -> Option<ContextId> {
        self.current_context
    }

    /// Get information about a specific context.
    pub fn get_context(&self, id: &ContextId) -> Option<&ExecutionContextInfo> {
        self.contexts.get(id)
    }

    /// Get all contexts in order of first appearance.
    pub fn all_contexts(&self) -> Vec<&ExecutionContextInfo> {
        self.contexts.values().collect()
    }

    /// Get the full switch history.
    pub fn switch_history(&self) -> &[(ContextId, u64)] {
        &self.switch_history
    }

    /// Total number of distinct contexts seen.
    pub fn context_count(&self) -> usize {
        self.contexts.len()
    }

    /// Check whether the current context is a kernel context.
    pub fn is_in_kernel(&self) -> bool {
        self.current_context
            .and_then(|id| self.contexts.get(&id))
            .is_some_and(|info| info.kind == ContextKind::Kernel)
    }

    /// Determine the context kind from a procedure name.
    ///
    /// Uses kernel procedure detection to classify the context.
    pub fn classify_from_procedure(name: &str) -> ContextKind {
        if kernel_procs::is_kernel_procedure(name) {
            ContextKind::Kernel
        } else {
            ContextKind::Unknown
        }
    }
}

impl Default for ExecutionContextTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Contract Trace Session
// ---------------------------------------------------------------------------

/// The status of a contract trace session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// Session created but chain not yet built.
    Created,
    /// Chain built, ready for transaction execution.
    ChainBuilt,
    /// Transaction executed, trace captured.
    Executed,
    /// Trace written to output files.
    Completed,
    /// Session failed with an error.
    Failed(String),
}

/// Manages the full lifecycle of tracing a contract-level test.
///
/// The lifecycle is:
/// 1. Create session with `new(config, out_dir)`
/// 2. Build the chain with `build_chain()`
/// 3. Execute a transaction with `execute_transaction(tx_config)`
/// 4. Finalize and write trace with `finalize()`
///
/// # Note on Current Implementation
///
/// Because `miden-testing` is not available at a compatible version, this
/// session operates on synthetic data. The execution step simulates context
/// switches and kernel procedure calls rather than running actual VM code.
/// When miden-testing becomes available, the `execute_transaction` method
/// will delegate to the real `TransactionContext::execute()` with our
/// `MidenTracer` attached to the `TransactionExecutor`.
pub struct ContractTraceSession {
    /// Chain configuration.
    config: MockChainConfig,
    /// Built chain state (populated after `build_chain()`).
    chain: Option<MockChainState>,
    /// Execution context tracker.
    context_tracker: ExecutionContextTracker,
    /// Output directory for trace files.
    out_dir: PathBuf,
    /// Current session status.
    status: SessionStatus,
    /// Collected trace events (synthetic, for testing).
    trace_events: Vec<TraceEvent>,
}

/// A synthetic trace event for testing purposes.
///
/// When the real miden-testing integration is available, these will be
/// replaced by actual CodeTracer trace events emitted by MidenTracer.
#[derive(Debug, Clone)]
pub enum TraceEvent {
    /// A step to a source location.
    Step {
        source_path: PathBuf,
        line: u32,
        context_id: ContextId,
    },
    /// A function call.
    Call {
        function_name: String,
        is_kernel: bool,
        context_id: ContextId,
    },
    /// A function return.
    Return {
        context_id: ContextId,
    },
    /// A variable value.
    Variable {
        name: String,
        value: String,
        context_id: ContextId,
    },
    /// A context switch.
    ContextSwitch {
        from: Option<ContextId>,
        to: ContextId,
        kind: ContextKind,
    },
}

impl ContractTraceSession {
    /// Create a new contract trace session.
    pub fn new(config: MockChainConfig, out_dir: PathBuf) -> Self {
        Self {
            config,
            chain: None,
            context_tracker: ExecutionContextTracker::new(),
            out_dir,
            status: SessionStatus::Created,
            trace_events: Vec::new(),
        }
    }

    /// Build the MockChain from the configuration.
    pub fn build_chain(&mut self) -> Result<(), String> {
        if self.status != SessionStatus::Created {
            return Err(format!(
                "Cannot build chain in state {:?}",
                self.status
            ));
        }

        let builder = MockChainBuilder::new();

        // Apply configuration to builder.
        let mut b = builder;
        for wallet in &self.config.wallets {
            b = b.add_existing_wallet(wallet.clone());
        }
        for faucet in &self.config.faucets {
            b = b.add_existing_basic_faucet(faucet.clone());
        }
        for note in &self.config.p2id_notes {
            b = b.add_p2id_note(note.clone());
        }
        for note in &self.config.output_notes {
            b = b.add_output_note(note.clone());
        }

        self.chain = Some(b.build());
        self.status = SessionStatus::ChainBuilt;
        Ok(())
    }

    /// Execute a transaction with tracing.
    ///
    /// This method simulates transaction execution with context switches
    /// and kernel procedure calls. When miden-testing is available, this
    /// will delegate to the real TransactionExecutor with our MidenTracer.
    pub fn execute_transaction(&mut self, tx_config: &TransactionConfig) -> Result<(), String> {
        if self.status != SessionStatus::ChainBuilt {
            return Err(format!(
                "Cannot execute transaction in state {:?}",
                self.status
            ));
        }

        let chain = self.chain.as_ref().ok_or("Chain not built")?;

        // Verify the account exists.
        if !chain.has_account(&tx_config.account_id) {
            return Err(format!(
                "Account {} not found in chain",
                tx_config.account_id
            ));
        }

        // Verify all input notes exist.
        for note_id in &tx_config.input_notes {
            if chain.get_note(note_id).is_none() {
                return Err(format!("Note {:?} not found in chain", note_id));
            }
        }

        // Simulate the transaction execution with context switches.
        // In a real implementation, this would call:
        //   let tx_inputs = chain.get_transaction_inputs(account_id, &note_ids, &[]);
        //   let tx_ctx = chain.build_tx_context(...).tx_script(script).build();
        //   let executed_tx = tx_ctx.execute().await;

        let mut clock = 0u64;

        // Phase 1: Kernel prologue (context 0).
        let kernel_ctx = ContextId(0);
        self.context_tracker
            .switch_context(kernel_ctx, ContextKind::Kernel, clock);
        self.trace_events.push(TraceEvent::ContextSwitch {
            from: None,
            to: kernel_ctx,
            kind: ContextKind::Kernel,
        });
        self.trace_events.push(TraceEvent::Call {
            function_name: "#sys::prologue".to_string(),
            is_kernel: true,
            context_id: kernel_ctx,
        });
        clock += 100;
        self.trace_events.push(TraceEvent::Return {
            context_id: kernel_ctx,
        });

        // Phase 2: Note script execution (one context per note).
        for (i, note_id) in tx_config.input_notes.iter().enumerate() {
            let note_ctx = ContextId((i + 1) as u32);
            self.context_tracker.switch_context(
                note_ctx,
                ContextKind::NoteScript {
                    note_id: *note_id,
                },
                clock,
            );
            self.trace_events.push(TraceEvent::ContextSwitch {
                from: Some(kernel_ctx),
                to: note_ctx,
                kind: ContextKind::NoteScript {
                    note_id: *note_id,
                },
            });
            self.trace_events.push(TraceEvent::Call {
                function_name: format!("note_script_{}", note_id.0),
                is_kernel: false,
                context_id: note_ctx,
            });

            // Simulate some steps in the note script.
            self.trace_events.push(TraceEvent::Step {
                source_path: PathBuf::from("note_script.masm"),
                line: 1,
                context_id: note_ctx,
            });
            self.trace_events.push(TraceEvent::Variable {
                name: "note_input_0".to_string(),
                value: "42".to_string(),
                context_id: note_ctx,
            });

            clock += 50;

            // Note script may call kernel procedures.
            self.context_tracker
                .switch_context(kernel_ctx, ContextKind::Kernel, clock);
            self.trace_events.push(TraceEvent::ContextSwitch {
                from: Some(note_ctx),
                to: kernel_ctx,
                kind: ContextKind::Kernel,
            });
            self.trace_events.push(TraceEvent::Call {
                function_name: "miden::note::get_inputs".to_string(),
                is_kernel: true,
                context_id: kernel_ctx,
            });
            clock += 20;
            self.trace_events.push(TraceEvent::Return {
                context_id: kernel_ctx,
            });

            // Return to note context.
            self.context_tracker.switch_context(
                note_ctx,
                ContextKind::NoteScript {
                    note_id: *note_id,
                },
                clock,
            );
            self.trace_events.push(TraceEvent::Return {
                context_id: note_ctx,
            });
            clock += 30;
        }

        // Phase 3: Account code execution (context N+1).
        let account_ctx = ContextId((tx_config.input_notes.len() + 1) as u32);
        self.context_tracker.switch_context(
            account_ctx,
            ContextKind::AccountCode {
                account_id: tx_config.account_id,
            },
            clock,
        );
        self.trace_events.push(TraceEvent::ContextSwitch {
            from: Some(kernel_ctx),
            to: account_ctx,
            kind: ContextKind::AccountCode {
                account_id: tx_config.account_id,
            },
        });
        self.trace_events.push(TraceEvent::Call {
            function_name: "account::receive_asset".to_string(),
            is_kernel: false,
            context_id: account_ctx,
        });
        self.trace_events.push(TraceEvent::Step {
            source_path: PathBuf::from("account.masm"),
            line: 5,
            context_id: account_ctx,
        });

        // Account code calls kernel to add asset.
        self.context_tracker
            .switch_context(kernel_ctx, ContextKind::Kernel, clock);
        self.trace_events.push(TraceEvent::Call {
            function_name: "miden::kernel::account_vault_add_asset".to_string(),
            is_kernel: true,
            context_id: kernel_ctx,
        });
        clock += 30;
        self.trace_events.push(TraceEvent::Return {
            context_id: kernel_ctx,
        });

        // Return to account context.
        self.context_tracker.switch_context(
            account_ctx,
            ContextKind::AccountCode {
                account_id: tx_config.account_id,
            },
            clock,
        );
        self.trace_events.push(TraceEvent::Return {
            context_id: account_ctx,
        });
        clock += 20;

        // Phase 4: Transaction script (if present).
        if tx_config.tx_script.is_some() {
            let tx_script_ctx = ContextId((tx_config.input_notes.len() + 2) as u32);
            self.context_tracker
                .switch_context(tx_script_ctx, ContextKind::TxScript, clock);
            self.trace_events.push(TraceEvent::ContextSwitch {
                from: Some(account_ctx),
                to: tx_script_ctx,
                kind: ContextKind::TxScript,
            });
            self.trace_events.push(TraceEvent::Call {
                function_name: "tx_script::main".to_string(),
                is_kernel: false,
                context_id: tx_script_ctx,
            });
            self.trace_events.push(TraceEvent::Step {
                source_path: PathBuf::from("tx_script.masm"),
                line: 1,
                context_id: tx_script_ctx,
            });
            clock += 40;
            self.trace_events.push(TraceEvent::Return {
                context_id: tx_script_ctx,
            });
        }

        // Phase 5: Kernel epilogue.
        self.context_tracker
            .switch_context(kernel_ctx, ContextKind::Kernel, clock);
        self.trace_events.push(TraceEvent::Call {
            function_name: "#sys::epilogue".to_string(),
            is_kernel: true,
            context_id: kernel_ctx,
        });
        self.trace_events.push(TraceEvent::Return {
            context_id: kernel_ctx,
        });

        // Record the transaction in the chain state.
        if let Some(chain) = self.chain.as_mut() {
            chain.record_transaction(TransactionRecord {
                account_id: tx_config.account_id,
                consumed_notes: tx_config.input_notes.clone(),
                success: true,
            });
        }

        self.status = SessionStatus::Executed;
        Ok(())
    }

    /// Finalize the session and write trace output.
    ///
    /// In the real implementation, this would call TraceWriter::finish_writing_*
    /// on the MidenTracer. Currently it writes a summary JSON file.
    pub fn finalize(&mut self) -> Result<PathBuf, String> {
        if self.status != SessionStatus::Executed {
            return Err(format!(
                "Cannot finalize in state {:?}",
                self.status
            ));
        }

        std::fs::create_dir_all(&self.out_dir)
            .map_err(|e| format!("Cannot create output dir: {e}"))?;

        let summary = self.build_summary();
        let summary_path = self.out_dir.join("contract_trace_summary.json");
        let json = serde_json::to_string_pretty(&summary)
            .map_err(|e| format!("JSON serialization failed: {e}"))?;
        std::fs::write(&summary_path, json)
            .map_err(|e| format!("Failed to write summary: {e}"))?;

        self.status = SessionStatus::Completed;
        Ok(summary_path)
    }

    /// Build a summary of the trace session for output.
    fn build_summary(&self) -> serde_json::Value {
        let context_count = self.context_tracker.context_count();
        let event_count = self.trace_events.len();

        let kernel_calls: Vec<&str> = self
            .trace_events
            .iter()
            .filter_map(|e| match e {
                TraceEvent::Call {
                    function_name,
                    is_kernel: true,
                    ..
                } => Some(function_name.as_str()),
                _ => None,
            })
            .collect();

        let context_switches: usize = self
            .trace_events
            .iter()
            .filter(|e| matches!(e, TraceEvent::ContextSwitch { .. }))
            .count();

        serde_json::json!({
            "session_status": format!("{:?}", self.status),
            "context_count": context_count,
            "event_count": event_count,
            "context_switches": context_switches,
            "kernel_calls": kernel_calls,
            "accounts": self.chain.as_ref().map(|c| c.account_count()).unwrap_or(0),
            "notes": self.chain.as_ref().map(|c| c.note_count()).unwrap_or(0),
        })
    }

    /// Get the current session status.
    pub fn status(&self) -> &SessionStatus {
        &self.status
    }

    /// Get a reference to the built chain state.
    pub fn chain(&self) -> Option<&MockChainState> {
        self.chain.as_ref()
    }

    /// Get the execution context tracker.
    pub fn context_tracker(&self) -> &ExecutionContextTracker {
        &self.context_tracker
    }

    /// Get the collected trace events.
    pub fn trace_events(&self) -> &[TraceEvent] {
        &self.trace_events
    }

    /// Get the output directory.
    pub fn out_dir(&self) -> &Path {
        &self.out_dir
    }
}

use serde;

/// Serialize support for summary output.
impl serde::Serialize for ContextKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ContextKind::Kernel => serializer.serialize_str("kernel"),
            ContextKind::AccountCode { account_id } => {
                serializer.serialize_str(&format!("account:{}", account_id))
            }
            ContextKind::NoteScript { note_id } => {
                serializer.serialize_str(&format!("note:{:?}", note_id))
            }
            ContextKind::TxScript => serializer.serialize_str("tx_script"),
            ContextKind::Unknown => serializer.serialize_str("unknown"),
        }
    }
}
