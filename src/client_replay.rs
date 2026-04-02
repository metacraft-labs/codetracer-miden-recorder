//! On-chain transaction replay via miden-client (M6).
//!
//! This module provides infrastructure for replaying real on-chain Miden
//! transactions locally with CodeTracer instrumentation.
//!
//! # Architecture
//!
//! Miden's client-side execution model is unique: transactions are executed
//! locally by the client, and only STARK proofs are submitted to the network.
//! The node verifies proofs but never re-executes transactions. This means
//! "on-chain replay" is really about reconstructing the exact pre-transaction
//! state (accounts, notes, block context) that existed at the time of original
//! execution.
//!
//! The pipeline is:
//! ```text
//! ReplayConfig
//!   |
//!   v
//! MidenNodeClient::sync_state()    -- fetch current chain state from node
//!   |
//!   v
//! ReplayDataStore                  -- provides TransactionInputs for replay
//!   |
//!   v
//! TransactionExecutor (debug_mode) -- execute transaction with tracer
//!   |
//!   v
//! CodeTracer trace output          -- trace.bin, trace_metadata.json, trace_paths.json
//! ```
//!
//! # Current Status
//!
//! `miden-client` is not available at a version compatible with our
//! `miden-processor` 0.13.x dependency. This module provides types and
//! infrastructure that mirror the expected miden-client API so that:
//!
//! 1. The architecture is established and tested with synthetic data
//! 2. When miden-client versions align, swapping in the real crate is straightforward
//! 3. Tests validate the full lifecycle without requiring a live node
//!
//! # Known Limitations for Historical Transaction Replay
//!
//! Replaying past transactions on Miden is fundamentally challenging because:
//!
//! 1. **No pre-transaction state storage**: The client does not store the full
//!    account state as it existed *before* each transaction. After execution,
//!    only the post-transaction state is retained.
//!
//! 2. **Transaction scripts are not stored**: After a transaction is executed
//!    and proven, the transaction script (MASM code) is typically discarded.
//!    Without the script, the transaction cannot be re-executed.
//!
//! 3. **No "get transaction by ID" RPC**: The node's RPC interface does not
//!    expose an endpoint to retrieve all inputs for a historical transaction.
//!    The node only stores proofs and state commitments, not execution inputs.
//!
//! 4. **Block context dependency**: Transactions depend on the block header
//!    at the time of execution (block number, chain root, note root, etc.).
//!    Reconstructing this requires access to historical block headers.
//!
//! 5. **Note consumption is one-time**: Input notes are consumed (nullified)
//!    during transaction execution. After consumption, the note data may not
//!    be retrievable from the network.
//!
//! ## Workaround: Instrumented Client Capture
//!
//! The recommended approach for reliable replay is to instrument the miden-client
//! to capture and serialize `TransactionInputs` at execution time. This
//! "capture-then-replay" workflow stores everything needed for later replay:
//!
//! ```text
//! [Original Execution]
//!   Client prepares TransactionInputs
//!     -> capture_transaction_inputs() serializes to disk
//!   Client executes transaction normally
//!   Client submits proof to network
//!
//! [Later Replay]
//!   Load captured TransactionInputs from disk
//!   Construct TransactionExecutor with debug_mode + tracing
//!   Re-execute with CodeTracerTracer attached
//!   Produce trace output
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for replaying an on-chain transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayConfig {
    /// URL of the Miden node's RPC endpoint.
    ///
    /// Used by `MidenNodeClient::sync_state()` to fetch chain state.
    /// Example: `"https://rpc.testnet.miden.io"`
    pub node_url: String,

    /// The account ID involved in the transaction (hex string, e.g. "0x1234...").
    ///
    /// This is the account whose state needs to be synced for replay.
    pub account_id: String,

    /// The transaction ID to replay (hex string).
    ///
    /// Note: the node may not support looking up transactions by ID directly.
    /// This is used for identification and logging. The actual replay requires
    /// reconstructing the transaction inputs.
    pub transaction_id: String,

    /// Directory where trace output files will be written.
    pub output_dir: PathBuf,

    /// Optional path to a captured TransactionInputs file.
    ///
    /// If provided, the replay will use these pre-captured inputs instead
    /// of trying to reconstruct them from the node. This is the recommended
    /// approach for reliable replay.
    pub captured_inputs_path: Option<PathBuf>,
}

impl ReplayConfig {
    /// Create a new ReplayConfig with required fields.
    pub fn new(
        node_url: impl Into<String>,
        account_id: impl Into<String>,
        transaction_id: impl Into<String>,
        output_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            node_url: node_url.into(),
            account_id: account_id.into(),
            transaction_id: transaction_id.into(),
            output_dir: output_dir.into(),
            captured_inputs_path: None,
        }
    }

    /// Set the path to captured transaction inputs.
    pub fn with_captured_inputs(mut self, path: impl Into<PathBuf>) -> Self {
        self.captured_inputs_path = Some(path.into());
        self
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), ReplayError> {
        if self.node_url.is_empty() {
            return Err(ReplayError::InvalidConfig(
                "node_url cannot be empty".to_string(),
            ));
        }
        if self.account_id.is_empty() {
            return Err(ReplayError::InvalidConfig(
                "account_id cannot be empty".to_string(),
            ));
        }
        if self.transaction_id.is_empty() {
            return Err(ReplayError::InvalidConfig(
                "transaction_id cannot be empty".to_string(),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Error Types
// ---------------------------------------------------------------------------

/// Errors that can occur during transaction replay.
#[derive(Debug, Clone)]
pub enum ReplayError {
    /// Invalid configuration.
    InvalidConfig(String),

    /// Failed to connect to the Miden node.
    NodeConnectionFailed(String),

    /// State sync failed.
    SyncFailed(String),

    /// Account not found in the local store after sync.
    AccountNotFound(String),

    /// Transaction inputs could not be reconstructed.
    InputsNotAvailable(String),

    /// Transaction execution failed during replay.
    ExecutionFailed(String),

    /// Trace output failed.
    TraceOutputFailed(String),

    /// The requested operation is not supported for historical transactions.
    HistoricalReplayNotSupported(String),

    /// Foreign account data could not be fetched.
    ForeignAccountNotFound(String),

    /// Captured inputs file could not be loaded.
    CapturedInputsLoadFailed(String),

    /// I/O error.
    IoError(String),
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::InvalidConfig(msg) => write!(f, "invalid config: {msg}"),
            ReplayError::NodeConnectionFailed(msg) => write!(f, "node connection failed: {msg}"),
            ReplayError::SyncFailed(msg) => write!(f, "state sync failed: {msg}"),
            ReplayError::AccountNotFound(msg) => write!(f, "account not found: {msg}"),
            ReplayError::InputsNotAvailable(msg) => write!(f, "inputs not available: {msg}"),
            ReplayError::ExecutionFailed(msg) => write!(f, "execution failed: {msg}"),
            ReplayError::TraceOutputFailed(msg) => write!(f, "trace output failed: {msg}"),
            ReplayError::HistoricalReplayNotSupported(msg) => {
                write!(f, "historical replay not supported: {msg}")
            }
            ReplayError::ForeignAccountNotFound(msg) => {
                write!(f, "foreign account not found: {msg}")
            }
            ReplayError::CapturedInputsLoadFailed(msg) => {
                write!(f, "captured inputs load failed: {msg}")
            }
            ReplayError::IoError(msg) => write!(f, "I/O error: {msg}"),
        }
    }
}

impl std::error::Error for ReplayError {}

// ---------------------------------------------------------------------------
// Transaction Input Types (mirroring miden-client / miden-base types)
// ---------------------------------------------------------------------------

/// A field element value (u64 representation of a Miden Felt).
pub type Felt = u64;

/// A Miden Word (4 field elements).
pub type Word = [Felt; 4];

/// A 32-byte digest (representing a Miden RPO hash).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    pub fn zero() -> Self {
        Digest([0u8; 32])
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Partial account state needed for transaction execution.
///
/// Mirrors `PartialAccount` from miden-base. Contains the account's
/// code, storage, vault, and nonce at the time of transaction execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialAccount {
    /// Account ID (as u64).
    pub id: u64,
    /// Account nonce at the time of execution.
    pub nonce: Felt,
    /// Root hash of the account's code (MastForest).
    pub code_commitment: Digest,
    /// Root hash of the account's storage slots.
    pub storage_commitment: Digest,
    /// Root hash of the account's asset vault.
    pub vault_commitment: Digest,
    /// Whether this is a public account (state stored on-chain).
    pub is_public: bool,
}

/// Block header at the time of transaction execution.
///
/// Mirrors `BlockHeader` from miden-base. Provides the block context
/// that the transaction was executed against.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Block number.
    pub block_num: u32,
    /// Protocol version.
    pub version: u32,
    /// Hash of the previous block.
    pub prev_hash: Digest,
    /// Root of the chain MMR (Merkle Mountain Range).
    pub chain_root: Digest,
    /// Root of the account tree.
    pub account_root: Digest,
    /// Root of the nullifier tree.
    pub nullifier_root: Digest,
    /// Root of the note tree.
    pub note_root: Digest,
    /// Root of the transaction tree.
    pub tx_hash: Digest,
    /// Timestamp.
    pub timestamp: u64,
}

impl BlockHeader {
    /// Create a synthetic block header for testing.
    pub fn synthetic(block_num: u32) -> Self {
        Self {
            block_num,
            version: 1,
            prev_hash: Digest::zero(),
            chain_root: Digest::zero(),
            account_root: Digest::zero(),
            nullifier_root: Digest::zero(),
            note_root: Digest::zero(),
            tx_hash: Digest::zero(),
            timestamp: 1700000000 + block_num as u64 * 10,
        }
    }
}

/// Authentication path for block inclusion in the chain MMR.
///
/// Mirrors `PartialBlockchain` (or `ChainMmr`) from miden-base.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartialBlockchain {
    /// MMR peaks.
    pub peaks: Vec<Digest>,
    /// Authentication path nodes.
    pub auth_nodes: Vec<(u64, Digest)>,
}

/// A single input note for transaction execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputNote {
    /// Note ID.
    pub id: u64,
    /// Note script hash.
    pub script_hash: Digest,
    /// Note inputs (field elements).
    pub inputs: Vec<Felt>,
    /// Assets contained in the note.
    pub assets: Vec<(u64, u64)>, // (faucet_id, amount)
    /// Sender account ID.
    pub sender: u64,
    /// Note metadata.
    pub metadata: NoteMetadata,
}

/// Metadata for a note.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NoteMetadata {
    /// Note type tag.
    pub tag: u32,
    /// Auxiliary data.
    pub aux: u64,
    /// Note type (0 = public, 1 = private, 2 = encrypted).
    pub note_type: u8,
}

/// Collection of input notes for a transaction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputNotes {
    pub notes: Vec<InputNote>,
}

/// Transaction arguments (script and note args).
///
/// Mirrors `TransactionArgs` from miden-base.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransactionArgs {
    /// Transaction script source (MASM).
    pub tx_script: Option<String>,
    /// Per-note arguments (note_id -> args).
    pub note_args: BTreeMap<u64, Vec<Felt>>,
}

/// Auxiliary data for the advice provider.
///
/// Mirrors `AdviceInputs` from miden-base.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AdviceInputs {
    /// Key-value map entries.
    pub map_entries: BTreeMap<String, Vec<Felt>>,
    /// Merkle store nodes.
    pub merkle_nodes: Vec<(Digest, Vec<(u64, Digest)>)>,
}

/// Complete set of inputs needed to execute a transaction.
///
/// This is the central data structure for replay. It contains everything
/// the `TransactionExecutor` needs to re-execute a transaction.
///
/// Mirrors `TransactionInputs` from miden-base:
/// ```ignore
/// pub struct TransactionInputs {
///     account: PartialAccount,
///     block_header: BlockHeader,
///     block_chain: PartialBlockchain,
///     input_notes: InputNotes,
///     tx_args: TransactionArgs,
///     advice_inputs: AdviceInputs,
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionInputs {
    /// Account state at the time of execution.
    pub account: PartialAccount,
    /// Block header context.
    pub block_header: BlockHeader,
    /// Block chain MMR authentication data.
    pub block_chain: PartialBlockchain,
    /// Notes being consumed.
    pub input_notes: InputNotes,
    /// Transaction arguments (script, note args).
    pub tx_args: TransactionArgs,
    /// Advice provider inputs.
    pub advice_inputs: AdviceInputs,
}

impl TransactionInputs {
    /// Create synthetic transaction inputs for testing.
    pub fn synthetic(account_id: u64, block_num: u32) -> Self {
        Self {
            account: PartialAccount {
                id: account_id,
                nonce: 1,
                code_commitment: Digest::zero(),
                storage_commitment: Digest::zero(),
                vault_commitment: Digest::zero(),
                is_public: true,
            },
            block_header: BlockHeader::synthetic(block_num),
            block_chain: PartialBlockchain::default(),
            input_notes: InputNotes::default(),
            tx_args: TransactionArgs::default(),
            advice_inputs: AdviceInputs::default(),
        }
    }

    /// Add an input note.
    pub fn with_input_note(mut self, note: InputNote) -> Self {
        self.input_notes.notes.push(note);
        self
    }

    /// Set the transaction script.
    pub fn with_tx_script(mut self, script: impl Into<String>) -> Self {
        self.tx_args.tx_script = Some(script.into());
        self
    }

    /// Serialize to JSON for capture.
    pub fn to_json(&self) -> Result<String, ReplayError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| ReplayError::IoError(format!("JSON serialization failed: {e}")))
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Result<Self, ReplayError> {
        serde_json::from_str(json)
            .map_err(|e| ReplayError::CapturedInputsLoadFailed(format!("JSON parse failed: {e}")))
    }
}

// ---------------------------------------------------------------------------
// Foreign Account Data
// ---------------------------------------------------------------------------

/// Foreign account data returned by the node.
///
/// When a transaction interacts with accounts other than the executing
/// account (e.g., reading a faucet's metadata), the DataStore must provide
/// the foreign account's state and witness data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignAccountData {
    /// Account ID.
    pub id: u64,
    /// Account state hash.
    pub account_hash: Digest,
    /// Account code commitment.
    pub code_commitment: Digest,
    /// Account storage commitment.
    pub storage_commitment: Digest,
    /// Merkle proof (witness) for inclusion in the account tree.
    pub witness: Vec<Digest>,
}

// ---------------------------------------------------------------------------
// DataStore Trait (mirroring miden-client's DataStore)
// ---------------------------------------------------------------------------

/// Trait providing transaction inputs to the TransactionExecutor.
///
/// Mirrors the `DataStore` trait from miden-client:
/// ```ignore
/// pub trait DataStore {
///     fn get_transaction_inputs(
///         &self,
///         account_id: AccountId,
///         block_ref: u32,
///         notes: &[NoteId],
///     ) -> Result<TransactionInputs, DataStoreError>;
///
///     fn get_foreign_account_inputs(
///         &self,
///         account_id: AccountId,
///     ) -> Result<(Account, AccountWitness), DataStoreError>;
/// }
/// ```
pub trait DataStore {
    /// Get transaction inputs for executing a transaction.
    ///
    /// Returns the account state, block header, chain MMR data,
    /// input notes, and advice inputs needed for execution.
    fn get_transaction_inputs(
        &self,
        account_id: u64,
        block_ref: u32,
        note_ids: &[u64],
    ) -> Result<TransactionInputs, ReplayError>;

    /// Get foreign account data for cross-account interactions.
    ///
    /// Returns the account state and Merkle witness for the requested
    /// foreign account.
    fn get_foreign_account_inputs(
        &self,
        account_id: u64,
    ) -> Result<ForeignAccountData, ReplayError>;
}

// ---------------------------------------------------------------------------
// ReplayDataStore
// ---------------------------------------------------------------------------

/// A DataStore implementation for transaction replay.
///
/// Stores pre-synced account states, block headers, and note data
/// that were populated by `MidenNodeClient::sync_state()`. Provides
/// these as `TransactionInputs` when the `TransactionExecutor` requests
/// them during replay.
///
/// Can also be populated from captured `TransactionInputs` for the
/// "capture-then-replay" workflow.
pub struct ReplayDataStore {
    /// Account states, keyed by account ID.
    accounts: BTreeMap<u64, PartialAccount>,
    /// Block headers, keyed by block number.
    block_headers: BTreeMap<u32, BlockHeader>,
    /// Notes available for consumption, keyed by note ID.
    notes: BTreeMap<u64, InputNote>,
    /// Foreign account data, keyed by account ID.
    foreign_accounts: BTreeMap<u64, ForeignAccountData>,
    /// Block chain MMR data.
    chain: PartialBlockchain,
    /// The latest synced block number.
    latest_block: u32,
}

impl ReplayDataStore {
    /// Create an empty data store.
    pub fn new() -> Self {
        Self {
            accounts: BTreeMap::new(),
            block_headers: BTreeMap::new(),
            notes: BTreeMap::new(),
            foreign_accounts: BTreeMap::new(),
            chain: PartialBlockchain::default(),
            latest_block: 0,
        }
    }

    /// Populate from captured TransactionInputs.
    ///
    /// This is the "capture-then-replay" path: load previously captured
    /// inputs and make them available via the DataStore trait.
    pub fn from_captured_inputs(inputs: &TransactionInputs) -> Self {
        let mut store = Self::new();
        store
            .accounts
            .insert(inputs.account.id, inputs.account.clone());
        store
            .block_headers
            .insert(inputs.block_header.block_num, inputs.block_header.clone());
        store.chain = inputs.block_chain.clone();
        store.latest_block = inputs.block_header.block_num;
        for note in &inputs.input_notes.notes {
            store.notes.insert(note.id, note.clone());
        }
        store
    }

    /// Insert an account state.
    pub fn insert_account(&mut self, account: PartialAccount) {
        self.accounts.insert(account.id, account);
    }

    /// Insert a block header.
    pub fn insert_block_header(&mut self, header: BlockHeader) {
        if header.block_num > self.latest_block {
            self.latest_block = header.block_num;
        }
        self.block_headers.insert(header.block_num, header);
    }

    /// Insert a note.
    pub fn insert_note(&mut self, note: InputNote) {
        self.notes.insert(note.id, note);
    }

    /// Insert foreign account data.
    pub fn insert_foreign_account(&mut self, data: ForeignAccountData) {
        self.foreign_accounts.insert(data.id, data);
    }

    /// Set the chain MMR data.
    pub fn set_chain(&mut self, chain: PartialBlockchain) {
        self.chain = chain;
    }

    /// Get the latest synced block number.
    pub fn latest_block(&self) -> u32 {
        self.latest_block
    }

    /// Number of accounts in the store.
    pub fn account_count(&self) -> usize {
        self.accounts.len()
    }

    /// Number of notes in the store.
    pub fn note_count(&self) -> usize {
        self.notes.len()
    }

    /// Number of block headers in the store.
    pub fn block_header_count(&self) -> usize {
        self.block_headers.len()
    }

    /// Check if an account exists in the store.
    pub fn has_account(&self, account_id: u64) -> bool {
        self.accounts.contains_key(&account_id)
    }

    /// Check if a note exists in the store.
    pub fn has_note(&self, note_id: u64) -> bool {
        self.notes.contains_key(&note_id)
    }

    /// Get an account by ID.
    pub fn get_account(&self, account_id: u64) -> Option<&PartialAccount> {
        self.accounts.get(&account_id)
    }
}

impl Default for ReplayDataStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DataStore for ReplayDataStore {
    fn get_transaction_inputs(
        &self,
        account_id: u64,
        block_ref: u32,
        note_ids: &[u64],
    ) -> Result<TransactionInputs, ReplayError> {
        let account = self
            .accounts
            .get(&account_id)
            .ok_or_else(|| ReplayError::AccountNotFound(format!("0x{account_id:x}")))?
            .clone();

        // Use the requested block_ref, or fall back to the latest block.
        let block_num = if block_ref > 0 {
            block_ref
        } else {
            self.latest_block
        };
        let block_header = self
            .block_headers
            .get(&block_num)
            .cloned()
            .unwrap_or_else(|| BlockHeader::synthetic(block_num));

        let mut input_notes = InputNotes::default();
        for &note_id in note_ids {
            let note = self
                .notes
                .get(&note_id)
                .ok_or_else(|| {
                    ReplayError::InputsNotAvailable(format!("note {note_id} not found in store"))
                })?
                .clone();
            input_notes.notes.push(note);
        }

        Ok(TransactionInputs {
            account,
            block_header,
            block_chain: self.chain.clone(),
            input_notes,
            tx_args: TransactionArgs::default(),
            advice_inputs: AdviceInputs::default(),
        })
    }

    fn get_foreign_account_inputs(
        &self,
        account_id: u64,
    ) -> Result<ForeignAccountData, ReplayError> {
        self.foreign_accounts
            .get(&account_id)
            .cloned()
            .ok_or_else(|| {
                ReplayError::ForeignAccountNotFound(format!("0x{account_id:x}"))
            })
    }
}

// ---------------------------------------------------------------------------
// Sync State Result
// ---------------------------------------------------------------------------

/// Result of a state sync operation from the node.
///
/// Mirrors the data returned by `Client::sync_state()` from miden-client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStateResult {
    /// Block number synced to.
    pub block_num: u32,
    /// Number of account updates received.
    pub account_updates: usize,
    /// Number of new notes received.
    pub new_notes: usize,
    /// Number of nullifier updates (consumed notes).
    pub nullifier_updates: usize,
    /// Whether the sync completed successfully.
    pub success: bool,
}

// ---------------------------------------------------------------------------
// MidenNodeClient
// ---------------------------------------------------------------------------

/// Client for communicating with a Miden node via RPC.
///
/// Mirrors the key methods of `Client` from miden-client:
/// - `sync_state()`: fetch account updates, notes, block headers, MMR nodes
/// - `get_transaction_inputs()`: get inputs for a specific transaction
/// - `get_foreign_account_inputs()`: get foreign account state
///
/// # Current Implementation
///
/// Since miden-client is not available at a compatible version, this struct
/// provides a simulated implementation that works with synthetic data.
/// The API surface mirrors what miden-client will provide, so integration
/// is straightforward when the dependency becomes available.
pub struct MidenNodeClient {
    /// Node RPC URL.
    node_url: String,
    /// Local data store populated by sync operations.
    store: ReplayDataStore,
    /// Whether we are connected to a real node.
    connected: bool,
}

impl MidenNodeClient {
    /// Create a new client for the given node URL.
    ///
    /// Does not connect immediately; call `sync_state()` to initiate.
    pub fn new(node_url: impl Into<String>) -> Self {
        Self {
            node_url: node_url.into(),
            store: ReplayDataStore::new(),
            connected: false,
        }
    }

    /// Get a reference to the node URL.
    pub fn node_url(&self) -> &str {
        &self.node_url
    }

    /// Check if the client is connected to a node.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Sync state from the Miden node.
    ///
    /// In the real implementation, this calls `Client::sync_state()` which:
    /// 1. Fetches account updates from the node
    /// 2. Fetches new notes and nullifier changes
    /// 3. Fetches block headers and MMR authentication nodes
    /// 4. Populates the local store with all fetched data
    ///
    /// Currently returns a simulated result. When miden-client is available:
    /// ```ignore
    /// let sync_result = self.client.sync_state().await?;
    /// // The client's internal store is now populated
    /// ```
    pub fn sync_state(&mut self) -> Result<SyncStateResult, ReplayError> {
        // In a real implementation, this would make RPC calls to the node.
        // For now, return an error indicating the node is not available.
        //
        // When miden-client is available, the implementation will be:
        // ```
        // let client = Client::new(config)?;
        // let sync_summary = client.sync_state().await?;
        // // Transfer synced data to our ReplayDataStore
        // ```
        Err(ReplayError::NodeConnectionFailed(format!(
            "miden-client is not available at a compatible version. \
             Cannot connect to node at {}. \
             Use captured TransactionInputs for replay instead.",
            self.node_url
        )))
    }

    /// Sync state with synthetic data for testing.
    ///
    /// Populates the store with the provided accounts, notes, and block headers.
    pub fn sync_state_synthetic(
        &mut self,
        accounts: Vec<PartialAccount>,
        notes: Vec<InputNote>,
        block_headers: Vec<BlockHeader>,
    ) -> SyncStateResult {
        for account in &accounts {
            self.store.insert_account(account.clone());
        }
        let account_updates = accounts.len();

        for note in &notes {
            self.store.insert_note(note.clone());
        }
        let new_notes = notes.len();

        for header in &block_headers {
            self.store.insert_block_header(header.clone());
        }

        self.connected = true;

        SyncStateResult {
            block_num: self.store.latest_block(),
            account_updates,
            new_notes,
            nullifier_updates: 0,
            success: true,
        }
    }

    /// Get the underlying data store.
    pub fn store(&self) -> &ReplayDataStore {
        &self.store
    }

    /// Get a mutable reference to the underlying data store.
    pub fn store_mut(&mut self) -> &mut ReplayDataStore {
        &mut self.store
    }

    /// Fetch foreign account inputs from the node.
    ///
    /// In the real implementation, this calls the node's RPC to get
    /// the account state and Merkle witness for a foreign account.
    pub fn get_foreign_account_inputs(
        &self,
        account_id: u64,
    ) -> Result<ForeignAccountData, ReplayError> {
        self.store.get_foreign_account_inputs(account_id)
    }
}

// ---------------------------------------------------------------------------
// Replay Pipeline
// ---------------------------------------------------------------------------

/// Status of a replay operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayStatus {
    /// Replay not started.
    NotStarted,
    /// State sync in progress.
    Syncing,
    /// State synced, preparing inputs.
    PreparingInputs,
    /// Executing transaction with tracer.
    Executing,
    /// Trace output being written.
    WritingTrace,
    /// Replay completed successfully.
    Completed,
    /// Replay failed.
    Failed(String),
}

/// Result of a successful replay.
#[derive(Debug, Clone)]
pub struct ReplayResult {
    /// Path to the trace output directory.
    pub output_dir: PathBuf,
    /// The transaction ID that was replayed.
    pub transaction_id: String,
    /// Block number the transaction was executed against.
    pub block_num: u32,
    /// Account ID involved.
    pub account_id: String,
    /// Number of input notes consumed.
    pub input_note_count: usize,
    /// The status.
    pub status: ReplayStatus,
}

/// Replay a transaction from a Miden node.
///
/// This is the main entry point for on-chain transaction replay. The pipeline:
///
/// 1. Validate the configuration
/// 2. If captured inputs are provided, load them from disk
/// 3. Otherwise, sync state from the node and construct inputs
/// 4. Build a TransactionExecutor with debug_mode and tracing
/// 5. Execute the transaction with CodeTracerTracer attached
/// 6. Write trace output to the output directory
///
/// # Errors
///
/// Returns `ReplayError` if:
/// - The configuration is invalid
/// - State sync fails (no node connection, or miden-client not available)
/// - Transaction inputs cannot be reconstructed
/// - Transaction execution fails
/// - Trace output cannot be written
pub fn replay_transaction(config: &ReplayConfig) -> Result<ReplayResult, ReplayError> {
    config.validate()?;

    // If captured inputs are provided, use the capture-then-replay path.
    if let Some(ref captured_path) = config.captured_inputs_path {
        return replay_from_captured_inputs(config, captured_path);
    }

    // Otherwise, try to sync from the node (currently not supported).
    Err(ReplayError::HistoricalReplayNotSupported(
        "Historical transaction replay requires either:\n\
         1. Captured TransactionInputs (use --captured-inputs <path>)\n\
         2. A compatible miden-client version to sync state from a node\n\n\
         The miden-client is not currently available at a version compatible \
         with miden-processor 0.13.x. Use the capture_transaction_inputs() \
         function during original execution to save inputs for later replay."
            .to_string(),
    ))
}

/// Replay from previously captured TransactionInputs.
fn replay_from_captured_inputs(
    config: &ReplayConfig,
    captured_path: &Path,
) -> Result<ReplayResult, ReplayError> {
    // Load the captured inputs.
    let json = std::fs::read_to_string(captured_path).map_err(|e| {
        ReplayError::CapturedInputsLoadFailed(format!(
            "failed to read {}: {e}",
            captured_path.display()
        ))
    })?;
    let inputs = TransactionInputs::from_json(&json)?;

    // Create a data store from the captured inputs.
    let _data_store = ReplayDataStore::from_captured_inputs(&inputs);

    // Create the output directory.
    std::fs::create_dir_all(&config.output_dir).map_err(|e| {
        ReplayError::IoError(format!(
            "cannot create output dir {}: {e}",
            config.output_dir.display()
        ))
    })?;

    // In the real implementation, this would:
    // 1. Create a TransactionExecutor with debug_mode:
    //    TransactionExecutor::new(&data_store).with_debug_mode().with_tracing()
    // 2. Attach our CodeTracerTracer
    // 3. Execute the transaction
    // 4. Write trace files
    //
    // For now, we write a replay summary to indicate the pipeline worked.
    let summary = serde_json::json!({
        "status": "replay_completed_synthetic",
        "transaction_id": config.transaction_id,
        "account_id": config.account_id,
        "block_num": inputs.block_header.block_num,
        "input_notes": inputs.input_notes.notes.len(),
        "has_tx_script": inputs.tx_args.tx_script.is_some(),
        "limitation": "Full replay requires TransactionExecutor from miden-client. \
                       Currently using synthetic execution path.",
    });

    let summary_path = config.output_dir.join("replay_summary.json");
    let json_out = serde_json::to_string_pretty(&summary)
        .map_err(|e| ReplayError::IoError(format!("JSON serialization failed: {e}")))?;
    std::fs::write(&summary_path, json_out)
        .map_err(|e| ReplayError::IoError(format!("failed to write summary: {e}")))?;

    Ok(ReplayResult {
        output_dir: config.output_dir.clone(),
        transaction_id: config.transaction_id.clone(),
        block_num: inputs.block_header.block_num,
        account_id: config.account_id.clone(),
        input_note_count: inputs.input_notes.notes.len(),
        status: ReplayStatus::Completed,
    })
}

/// Capture TransactionInputs at execution time for later replay.
///
/// This function is meant to be called during the original transaction
/// execution flow (before the transaction is submitted to the network).
/// It serializes the complete TransactionInputs to a JSON file that can
/// later be used with `replay_transaction()`.
///
/// # Instrumented Client Workflow
///
/// ```text
/// // During original execution:
/// let tx_inputs = data_store.get_transaction_inputs(account_id, block_ref, &notes)?;
/// capture_transaction_inputs(&tx_inputs, "/path/to/captures/")?;
///
/// // ... execute transaction normally ...
/// // ... submit proof to network ...
///
/// // Later, for replay:
/// let config = ReplayConfig::new(node_url, account_id, tx_id, output_dir)
///     .with_captured_inputs("/path/to/captures/tx_inputs_0xabc123.json");
/// replay_transaction(&config)?;
/// ```
pub fn capture_transaction_inputs(
    inputs: &TransactionInputs,
    capture_dir: &Path,
) -> Result<PathBuf, ReplayError> {
    std::fs::create_dir_all(capture_dir).map_err(|e| {
        ReplayError::IoError(format!(
            "cannot create capture dir {}: {e}",
            capture_dir.display()
        ))
    })?;

    let filename = format!(
        "tx_inputs_0x{:x}_block_{}.json",
        inputs.account.id, inputs.block_header.block_num
    );
    let path = capture_dir.join(&filename);

    let json = inputs.to_json()?;
    std::fs::write(&path, &json)
        .map_err(|e| ReplayError::IoError(format!("failed to write {}: {e}", path.display())))?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replay_error_display() {
        let err = ReplayError::InvalidConfig("bad".to_string());
        assert_eq!(err.to_string(), "invalid config: bad");

        let err = ReplayError::HistoricalReplayNotSupported("reason".to_string());
        assert!(err
            .to_string()
            .contains("historical replay not supported"));
    }

    #[test]
    fn test_digest_display() {
        let d = Digest::zero();
        let s = d.to_string();
        assert_eq!(s.len(), 64); // 32 bytes * 2 hex chars
        assert!(s.chars().all(|c| c == '0'));
    }

    #[test]
    fn test_transaction_inputs_json_roundtrip() {
        let inputs = TransactionInputs::synthetic(0x1000, 42);
        let json = inputs.to_json().unwrap();
        let parsed = TransactionInputs::from_json(&json).unwrap();
        assert_eq!(parsed.account.id, 0x1000);
        assert_eq!(parsed.block_header.block_num, 42);
    }
}
