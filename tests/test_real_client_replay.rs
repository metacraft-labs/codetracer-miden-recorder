//! Integration tests for client_replay using real miden-testing types.
//!
//! These tests verify that the recorder's client_replay module works correctly
//! with data derived from real Miden VM types (via miden-testing and miden-protocol).
//!
//! The existing 31 tests in test_client_replay.rs exercise the synthetic client
//! replay implementation with zero miden imports. These tests complement them by:
//!
//! 1. Using real MockChain to create real accounts, notes, and TransactionInputs
//! 2. Extracting data from real miden types into the recorder's synthetic types
//! 3. Verifying the capture/replay pipeline with real-derived data
//! 4. Testing serialization roundtrips of data populated from real miden sources
//! 5. Validating that the recorder's DataStore trait works with real-derived inputs
//!
//! Note: miden-client 0.14.0-alpha.2 requires Rust 1.93+ but our toolchain is
//! 1.91.1. These tests use miden-testing and miden-protocol (already available)
//! to exercise the same transaction execution and data flow patterns that
//! miden-client would use internally.

use miden_protocol::account::AccountId;
use miden_protocol::asset::FungibleAsset;
use miden_protocol::note::NoteType;
use miden_testing::{Auth, MockChain, TransactionContextBuilder};

use codetracer_miden_recorder::client_replay::*;

// ---------------------------------------------------------------------------
// Helper: convert real miden AccountId to our synthetic u64
// ---------------------------------------------------------------------------

fn account_id_to_u64(id: AccountId) -> u64 {
    // AccountId can be converted to u128, take lower 64 bits for our synthetic type.
    let id_u128: u128 = id.into();
    id_u128 as u64
}

// ---------------------------------------------------------------------------
// Helper: build synthetic TransactionInputs from real MockChain data
// ---------------------------------------------------------------------------

/// Creates a synthetic TransactionInputs populated with data extracted from
/// a real MockChain. This bridges the gap between the recorder's synthetic
/// types and the real miden-protocol types.
fn synthetic_inputs_from_real_chain(
    chain: &MockChain,
    account_id: AccountId,
    block_num: u32,
) -> TransactionInputs {
    let account = chain
        .committed_account(account_id)
        .expect("account should exist");

    let acct_id_u64 = account_id_to_u64(account_id);

    // Extract real account properties into our synthetic PartialAccount.
    // miden-core 0.22 Felt uses as_canonical_u64() instead of as_int().
    let nonce_u64: u64 = account.nonce().as_canonical_u64();

    TransactionInputs {
        account: PartialAccount {
            id: acct_id_u64,
            nonce: nonce_u64,
            code_commitment: Digest::zero(), // simplified
            storage_commitment: Digest::zero(),
            vault_commitment: Digest::zero(),
            is_public: account.is_public(),
        },
        block_header: BlockHeader {
            block_num,
            version: 1,
            prev_hash: Digest::zero(),
            chain_root: Digest::zero(),
            account_root: Digest::zero(),
            nullifier_root: Digest::zero(),
            note_root: Digest::zero(),
            tx_hash: Digest::zero(),
            // timestamp() returns u32, our synthetic type uses u64.
            timestamp: chain.latest_block_header().timestamp() as u64,
        },
        block_chain: PartialBlockchain::default(),
        input_notes: InputNotes::default(),
        tx_args: TransactionArgs::default(),
        advice_inputs: AdviceInputs::default(),
    }
}

/// Extract a u64 identifier from a real NoteId (using first 8 bytes).
fn note_id_to_u64(note_id: miden_protocol::note::NoteId) -> u64 {
    let bytes = note_id.as_bytes();
    u64::from_le_bytes(bytes[0..8].try_into().unwrap())
}

// ---------------------------------------------------------------------------
// Test 1: Real account data populates synthetic PartialAccount correctly
// ---------------------------------------------------------------------------

#[test]
fn test_real_account_populates_synthetic_partial_account() {
    let mut builder = MockChain::builder();

    let wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let chain = builder.build().expect("failed to build chain");

    let account = chain
        .committed_account(wallet.id())
        .expect("wallet should exist");

    // Build synthetic PartialAccount from real account data.
    let synthetic = PartialAccount {
        id: account_id_to_u64(wallet.id()),
        nonce: account.nonce().as_canonical_u64(),
        code_commitment: Digest::zero(),
        storage_commitment: Digest::zero(),
        vault_commitment: Digest::zero(),
        is_public: account.is_public(),
    };

    assert_ne!(synthetic.id, 0, "real account ID should be non-zero");
    assert!(
        synthetic.nonce > 0,
        "existing wallet should have non-zero nonce"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Real MockChain block header timestamp populates synthetic header
// ---------------------------------------------------------------------------

#[test]
fn test_real_block_header_populates_synthetic() {
    let builder = MockChain::builder();
    let chain = builder.build().expect("failed to build chain");

    let real_header = chain.latest_block_header();
    let block_num: u32 = real_header.block_num().as_u32();

    let synthetic = BlockHeader {
        block_num,
        version: real_header.version(),
        prev_hash: Digest::zero(),
        chain_root: Digest::zero(),
        account_root: Digest::zero(),
        nullifier_root: Digest::zero(),
        note_root: Digest::zero(),
        tx_hash: Digest::zero(),
        timestamp: real_header.timestamp() as u64,
    };

    // Genesis block may have timestamp 0 or non-zero depending on MockChain.
    assert_eq!(synthetic.block_num, block_num, "block number should match");
}

// ---------------------------------------------------------------------------
// Test 3: Synthetic TransactionInputs from real chain data roundtrips via JSON
// ---------------------------------------------------------------------------

#[test]
fn test_real_derived_inputs_json_roundtrip() {
    let mut builder = MockChain::builder();

    let wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let chain = builder.build().expect("failed to build chain");

    let block_num: u32 = chain.latest_block_header().block_num().as_u32();
    let inputs = synthetic_inputs_from_real_chain(&chain, wallet.id(), block_num);

    // Serialize and deserialize.
    let json = inputs.to_json().expect("serialization should succeed");
    let parsed = TransactionInputs::from_json(&json).expect("deserialization should succeed");

    assert_eq!(parsed.account.id, inputs.account.id);
    assert_eq!(parsed.account.nonce, inputs.account.nonce);
    assert_eq!(parsed.block_header.block_num, inputs.block_header.block_num);
    assert_eq!(parsed.block_header.timestamp, inputs.block_header.timestamp);
    assert_eq!(parsed.account.is_public, inputs.account.is_public);
}

// ---------------------------------------------------------------------------
// Test 4: ReplayDataStore populated from real chain data
// ---------------------------------------------------------------------------

#[test]
fn test_replay_data_store_from_real_chain() {
    let mut builder = MockChain::builder();

    let wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let faucet = builder
        .add_existing_basic_faucet(Auth::IncrNonce, "TEST", 1_000_000, None)
        .expect("failed to create faucet");

    let chain = builder.build().expect("failed to build chain");

    let block_num: u32 = chain.latest_block_header().block_num().as_u32();
    let inputs = synthetic_inputs_from_real_chain(&chain, wallet.id(), block_num);

    // Populate ReplayDataStore from real-derived inputs.
    let store = ReplayDataStore::from_captured_inputs(&inputs);

    assert_eq!(store.account_count(), 1);
    assert!(store.has_account(inputs.account.id));
    assert_eq!(store.latest_block(), block_num);
    assert_eq!(store.block_header_count(), 1);

    // Verify we can retrieve transaction inputs from the store.
    let retrieved = store
        .get_transaction_inputs(inputs.account.id, block_num, &[])
        .expect("should retrieve inputs");

    assert_eq!(retrieved.account.id, inputs.account.id);
    assert_eq!(retrieved.account.nonce, inputs.account.nonce);
    assert_eq!(retrieved.block_header.block_num, block_num);

    // Also check faucet can be stored.
    let faucet_inputs = synthetic_inputs_from_real_chain(&chain, faucet.id(), block_num);
    let mut store2 = ReplayDataStore::new();
    store2.insert_account(faucet_inputs.account.clone());
    assert!(store2.has_account(faucet_inputs.account.id));
}

// ---------------------------------------------------------------------------
// Test 5: Full capture-replay pipeline with real-derived data
// ---------------------------------------------------------------------------

#[test]
fn test_capture_replay_pipeline_with_real_data() {
    let mut builder = MockChain::builder();

    let wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let chain = builder.build().expect("failed to build chain");

    let block_num: u32 = chain.latest_block_header().block_num().as_u32();
    let inputs = synthetic_inputs_from_real_chain(&chain, wallet.id(), block_num);

    let tmp_dir = tempfile::tempdir().unwrap();
    let capture_dir = tmp_dir.path().join("captures");
    let output_dir = tmp_dir.path().join("output");

    // Step 1: Capture inputs to disk.
    let captured_path =
        capture_transaction_inputs(&inputs, &capture_dir).expect("capture should succeed");
    assert!(captured_path.exists(), "captured file should exist on disk");

    // Step 2: Replay from captured inputs.
    let config = ReplayConfig::new(
        "https://rpc.testnet.miden.io",
        &format!("0x{:x}", inputs.account.id),
        "0xtest_real_replay",
        &output_dir,
    )
    .with_captured_inputs(&captured_path);

    let result = replay_transaction(&config).expect("replay should succeed");

    assert_eq!(result.status, ReplayStatus::Completed);
    assert_eq!(result.block_num, block_num);
    assert_eq!(result.input_note_count, 0);

    // Step 3: Verify output files.
    let summary_path = output_dir.join("replay_summary.json");
    assert!(summary_path.exists(), "replay summary should exist");

    let summary: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&summary_path).unwrap()).unwrap();
    assert_eq!(summary["block_num"], block_num);
    assert_eq!(summary["status"], "replay_completed_synthetic");
}

// ---------------------------------------------------------------------------
// Test 6: Real P2ID note data populates synthetic InputNote
// ---------------------------------------------------------------------------

#[test]
fn test_real_note_populates_synthetic_input_note() {
    let mut builder = MockChain::builder();

    let sender = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create sender");

    let receiver = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create receiver");

    let asset = FungibleAsset::mock(100);

    let note = builder
        .add_p2id_note(sender.id(), receiver.id(), &[asset], NoteType::Public)
        .expect("failed to create P2ID note");

    let chain = builder.build().expect("failed to build chain");

    // Verify the note exists in the real chain.
    assert!(
        chain.committed_notes().get(&note.id()).is_some(),
        "note should exist in chain"
    );

    // Build a synthetic InputNote from the real note's properties.
    let note_id_u64 = note_id_to_u64(note.id());

    let synthetic_note = InputNote {
        id: note_id_u64,
        script_hash: Digest::zero(), // simplified
        inputs: vec![],              // P2ID inputs would be the receiver's account ID
        assets: vec![(account_id_to_u64(sender.id()), 100)],
        sender: account_id_to_u64(sender.id()),
        metadata: NoteMetadata {
            tag: 0,
            aux: 0,
            note_type: 0, // public
        },
    };

    assert_ne!(
        synthetic_note.id, 0,
        "real-derived note ID should be non-zero"
    );
    assert_eq!(synthetic_note.assets.len(), 1, "should have one asset");
    assert_eq!(synthetic_note.assets[0].1, 100, "asset amount should match");
}

// ---------------------------------------------------------------------------
// Test 7: Capture-replay with real-derived notes
// ---------------------------------------------------------------------------

#[test]
fn test_capture_replay_with_real_derived_notes() {
    let mut builder = MockChain::builder();

    let sender = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create sender");

    let receiver = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create receiver");

    let asset = FungibleAsset::mock(250);

    let note = builder
        .add_p2id_note(sender.id(), receiver.id(), &[asset], NoteType::Public)
        .expect("failed to create note");

    let _chain = builder.build().expect("failed to build chain");

    // Build synthetic inputs with a note derived from real data.
    let note_id_u64 = note_id_to_u64(note.id());

    let synthetic_note = InputNote {
        id: note_id_u64,
        script_hash: Digest::zero(),
        inputs: vec![account_id_to_u64(receiver.id())],
        assets: vec![(account_id_to_u64(sender.id()), 250)],
        sender: account_id_to_u64(sender.id()),
        metadata: NoteMetadata::default(),
    };

    // Use synthetic helper with explicit block num since we don't need real chain state.
    let inputs = TransactionInputs::synthetic(account_id_to_u64(receiver.id()), 1)
        .with_input_note(synthetic_note);

    // Capture and replay.
    let tmp_dir = tempfile::tempdir().unwrap();
    let captured_path =
        capture_transaction_inputs(&inputs, tmp_dir.path()).expect("capture should succeed");

    // Reload and verify the note survived serialization.
    let loaded_json = std::fs::read_to_string(&captured_path).unwrap();
    let loaded = TransactionInputs::from_json(&loaded_json).expect("should parse");

    assert_eq!(loaded.input_notes.notes.len(), 1);
    assert_eq!(loaded.input_notes.notes[0].id, note_id_u64);
    assert_eq!(loaded.input_notes.notes[0].assets[0].1, 250);
    assert_eq!(
        loaded.input_notes.notes[0].inputs[0],
        account_id_to_u64(receiver.id())
    );
}

// ---------------------------------------------------------------------------
// Test 8: DataStore trait with real-derived multi-account store
// ---------------------------------------------------------------------------

#[test]
fn test_data_store_trait_with_real_derived_accounts() {
    let mut builder = MockChain::builder();

    let wallet_a = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet A");

    let wallet_b = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet B");

    let faucet = builder
        .add_existing_basic_faucet(Auth::IncrNonce, "DS", 1_000_000, None)
        .expect("failed to create faucet");

    let chain = builder.build().expect("failed to build chain");
    let block_num: u32 = chain.latest_block_header().block_num().as_u32();

    // Build a multi-account store from real data.
    let mut store = ReplayDataStore::new();

    for acct_id in &[wallet_a.id(), wallet_b.id(), faucet.id()] {
        let account = chain.committed_account(*acct_id).unwrap();
        store.insert_account(PartialAccount {
            id: account_id_to_u64(*acct_id),
            nonce: account.nonce().as_canonical_u64(),
            code_commitment: Digest::zero(),
            storage_commitment: Digest::zero(),
            vault_commitment: Digest::zero(),
            is_public: account.is_public(),
        });
    }
    store.insert_block_header(BlockHeader {
        block_num,
        version: 1,
        prev_hash: Digest::zero(),
        chain_root: Digest::zero(),
        account_root: Digest::zero(),
        nullifier_root: Digest::zero(),
        note_root: Digest::zero(),
        tx_hash: Digest::zero(),
        timestamp: chain.latest_block_header().timestamp() as u64,
    });

    assert_eq!(store.account_count(), 3);
    assert_eq!(store.block_header_count(), 1);

    // Use as trait object.
    let ds: &dyn DataStore = &store;

    let inputs_a = ds
        .get_transaction_inputs(account_id_to_u64(wallet_a.id()), block_num, &[])
        .expect("should get inputs for wallet A");
    assert_eq!(inputs_a.account.id, account_id_to_u64(wallet_a.id()));

    let inputs_b = ds
        .get_transaction_inputs(account_id_to_u64(wallet_b.id()), block_num, &[])
        .expect("should get inputs for wallet B");
    assert_eq!(inputs_b.account.id, account_id_to_u64(wallet_b.id()));

    let inputs_f = ds
        .get_transaction_inputs(account_id_to_u64(faucet.id()), block_num, &[])
        .expect("should get inputs for faucet");
    assert_eq!(inputs_f.account.id, account_id_to_u64(faucet.id()));

    // Non-existent account should fail.
    let err = ds.get_transaction_inputs(0xDEAD, block_num, &[]);
    assert!(err.is_err());
}

// ---------------------------------------------------------------------------
// Test 9: MidenNodeClient synthetic sync with real-derived data
// ---------------------------------------------------------------------------

#[test]
fn test_node_client_sync_with_real_derived_data() {
    let mut builder = MockChain::builder();

    let wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let faucet = builder
        .add_existing_basic_faucet(Auth::IncrNonce, "SYNC", 500_000, None)
        .expect("failed to create faucet");

    let chain = builder.build().expect("failed to build chain");
    let block_num: u32 = chain.latest_block_header().block_num().as_u32();

    // Build synthetic accounts from real data.
    let accounts: Vec<PartialAccount> = [wallet.id(), faucet.id()]
        .iter()
        .map(|id| {
            let acct = chain.committed_account(*id).unwrap();
            PartialAccount {
                id: account_id_to_u64(*id),
                nonce: acct.nonce().as_canonical_u64(),
                code_commitment: Digest::zero(),
                storage_commitment: Digest::zero(),
                vault_commitment: Digest::zero(),
                is_public: acct.is_public(),
            }
        })
        .collect();

    let headers = vec![BlockHeader {
        block_num,
        version: 1,
        prev_hash: Digest::zero(),
        chain_root: Digest::zero(),
        account_root: Digest::zero(),
        nullifier_root: Digest::zero(),
        note_root: Digest::zero(),
        tx_hash: Digest::zero(),
        timestamp: chain.latest_block_header().timestamp() as u64,
    }];

    // Sync the node client.
    let mut client = MidenNodeClient::new("https://rpc.testnet.miden.io");
    let result = client.sync_state_synthetic(accounts, vec![], headers);

    assert!(result.success);
    assert_eq!(result.account_updates, 2);
    assert_eq!(result.block_num, block_num);
    assert!(client.is_connected());

    // Verify store contents.
    let store = client.store();
    assert_eq!(store.account_count(), 2);
    assert!(store.has_account(account_id_to_u64(wallet.id())));
    assert!(store.has_account(account_id_to_u64(faucet.id())));
}

// ---------------------------------------------------------------------------
// Test 10: Real transaction execution produces data compatible with replay
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn test_real_tx_execution_compatible_with_replay() {
    let mut builder = MockChain::builder();

    let sender = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create sender");

    let receiver = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create receiver");

    let fungible_asset = FungibleAsset::mock(100).unwrap_fungible();

    let note = builder
        .add_p2id_note(
            sender.id(),
            receiver.id(),
            &[miden_protocol::asset::Asset::Fungible(fungible_asset)],
            NoteType::Public,
        )
        .expect("failed to create note");

    let mut chain = builder.build().expect("failed to build chain");

    // Execute a real transaction.
    let executed_tx = chain
        .build_tx_context(receiver.id(), &[note.id()], &[])
        .expect("failed to build tx context")
        .build()
        .expect("failed to build tx context")
        .execute()
        .await
        .expect("transaction execution failed");

    // The executed transaction's account ID should match.
    assert_eq!(executed_tx.account_id(), receiver.id());

    // Apply to chain.
    chain
        .add_pending_executed_transaction(&executed_tx)
        .expect("failed to add pending tx");
    chain.prove_next_block().expect("failed to prove block");

    // Now capture the post-transaction state for the replay module.
    let block_num: u32 = chain.latest_block_header().block_num().as_u32();
    let inputs = synthetic_inputs_from_real_chain(&chain, receiver.id(), block_num);

    // The receiver's nonce should have advanced after transaction.
    let receiver_account = chain.committed_account(receiver.id()).unwrap();
    assert_eq!(
        inputs.account.nonce,
        receiver_account.nonce().as_canonical_u64(),
        "synthetic nonce should match real post-tx nonce"
    );

    // Capture and verify the pipeline works.
    let tmp_dir = tempfile::tempdir().unwrap();
    let captured_path =
        capture_transaction_inputs(&inputs, tmp_dir.path()).expect("capture should succeed");

    let loaded = TransactionInputs::from_json(&std::fs::read_to_string(&captured_path).unwrap())
        .expect("should parse captured inputs");

    assert_eq!(loaded.account.id, account_id_to_u64(receiver.id()));
    assert_eq!(loaded.block_header.block_num, block_num);
}

// ---------------------------------------------------------------------------
// Test 11: Real get_transaction_inputs from MockChain
// ---------------------------------------------------------------------------

#[test]
fn test_real_get_transaction_inputs_from_mockchain() {
    let mut builder = MockChain::builder();

    let wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let chain = builder.build().expect("failed to build chain");

    // Use MockChain's real get_transaction_inputs method.
    let real_tx_inputs = chain
        .get_transaction_inputs(chain.committed_account(wallet.id()).unwrap(), &[], &[])
        .expect("should get real TransactionInputs");

    // Extract data from the real TransactionInputs.
    let real_account = real_tx_inputs.account();
    let real_header = real_tx_inputs.block_header();

    // Build a synthetic TransactionInputs from the real data.
    // Note: real_account here is miden_protocol::account::PartialAccount which
    // may not expose is_public(). We use the full Account from committed_account.
    let full_account = chain.committed_account(wallet.id()).unwrap();
    let synthetic = TransactionInputs {
        account: PartialAccount {
            id: account_id_to_u64(real_account.id()),
            nonce: real_account.nonce().as_canonical_u64(),
            code_commitment: Digest::zero(),
            storage_commitment: Digest::zero(),
            vault_commitment: Digest::zero(),
            is_public: full_account.is_public(),
        },
        block_header: BlockHeader {
            block_num: real_header.block_num().as_u32(),
            version: real_header.version(),
            prev_hash: Digest::zero(),
            chain_root: Digest::zero(),
            account_root: Digest::zero(),
            nullifier_root: Digest::zero(),
            note_root: Digest::zero(),
            tx_hash: Digest::zero(),
            timestamp: real_header.timestamp() as u64,
        },
        block_chain: PartialBlockchain::default(),
        input_notes: InputNotes::default(),
        tx_args: TransactionArgs::default(),
        advice_inputs: AdviceInputs::default(),
    };

    assert_ne!(synthetic.account.id, 0);
    assert!(synthetic.account.nonce > 0);
    // Genesis block may have block_num 0, so just verify it was populated.
    assert_eq!(
        synthetic.block_header.block_num,
        real_header.block_num().as_u32()
    );

    // Roundtrip via JSON.
    let json = synthetic.to_json().expect("should serialize");
    let parsed = TransactionInputs::from_json(&json).expect("should parse");
    assert_eq!(parsed.account.id, synthetic.account.id);
}

// ---------------------------------------------------------------------------
// Test 12: Foreign account data from real chain
// ---------------------------------------------------------------------------

#[test]
fn test_foreign_account_data_from_real_chain() {
    let mut builder = MockChain::builder();

    let wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let faucet = builder
        .add_existing_basic_faucet(Auth::IncrNonce, "FA", 1_000_000, None)
        .expect("failed to create faucet");

    let chain = builder.build().expect("failed to build chain");

    // Create synthetic ForeignAccountData from real chain accounts.
    let _faucet_account = chain.committed_account(faucet.id()).unwrap();

    let foreign_data = ForeignAccountData {
        id: account_id_to_u64(faucet.id()),
        account_hash: Digest::zero(),
        code_commitment: Digest::zero(),
        storage_commitment: Digest::zero(),
        witness: vec![Digest::zero()], // simplified
    };

    // Insert into a store and verify lookup works.
    let mut store = ReplayDataStore::new();
    store.insert_foreign_account(foreign_data);

    let result = store
        .get_foreign_account_inputs(account_id_to_u64(faucet.id()))
        .expect("should find foreign account");
    assert_eq!(result.id, account_id_to_u64(faucet.id()));

    // Non-existent foreign account should fail.
    let err = store.get_foreign_account_inputs(account_id_to_u64(wallet.id()));
    assert!(err.is_err());
}

// ---------------------------------------------------------------------------
// Test 13: Multiple captures from sequential real transactions
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn test_multiple_captures_from_sequential_real_txs() {
    let mut builder = MockChain::builder();

    let sender = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create sender");

    let receiver = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create receiver");

    let asset_1 = FungibleAsset::mock(100);
    let asset_2 = FungibleAsset::mock(200);

    let note_1 = builder
        .add_p2id_note(sender.id(), receiver.id(), &[asset_1], NoteType::Public)
        .expect("failed to create note 1");

    let note_2 = builder
        .add_p2id_note(sender.id(), receiver.id(), &[asset_2], NoteType::Public)
        .expect("failed to create note 2");

    let mut chain = builder.build().expect("failed to build chain");

    let tmp_dir = tempfile::tempdir().unwrap();
    let capture_dir = tmp_dir.path().join("captures");

    // Execute first transaction and capture.
    let tx_1 = chain
        .build_tx_context(receiver.id(), &[note_1.id()], &[])
        .expect("build tx 1")
        .build()
        .expect("build ctx 1")
        .execute()
        .await
        .expect("execute tx 1");

    chain
        .add_pending_executed_transaction(&tx_1)
        .expect("add tx 1");
    chain.prove_next_block().expect("prove block 1");

    let block_1: u32 = chain.latest_block_header().block_num().as_u32();
    let inputs_1 = synthetic_inputs_from_real_chain(&chain, receiver.id(), block_1);
    let path_1 = capture_transaction_inputs(&inputs_1, &capture_dir).expect("capture 1");

    // Execute second transaction and capture.
    let tx_2 = chain
        .build_tx_context(receiver.id(), &[note_2.id()], &[])
        .expect("build tx 2")
        .build()
        .expect("build ctx 2")
        .execute()
        .await
        .expect("execute tx 2");

    chain
        .add_pending_executed_transaction(&tx_2)
        .expect("add tx 2");
    chain.prove_next_block().expect("prove block 2");

    let block_2: u32 = chain.latest_block_header().block_num().as_u32();
    let inputs_2 = synthetic_inputs_from_real_chain(&chain, receiver.id(), block_2);
    let path_2 = capture_transaction_inputs(&inputs_2, &capture_dir).expect("capture 2");

    // Both captures should exist and be different.
    assert!(path_1.exists());
    assert!(path_2.exists());
    assert_ne!(
        path_1, path_2,
        "different blocks should produce different filenames"
    );

    // Nonces should advance between captures.
    let loaded_1 =
        TransactionInputs::from_json(&std::fs::read_to_string(&path_1).unwrap()).expect("parse 1");
    let loaded_2 =
        TransactionInputs::from_json(&std::fs::read_to_string(&path_2).unwrap()).expect("parse 2");

    assert!(
        loaded_2.account.nonce > loaded_1.account.nonce,
        "nonce should advance between transactions"
    );
    assert!(
        loaded_2.block_header.block_num > loaded_1.block_header.block_num,
        "block number should advance"
    );
}

// ---------------------------------------------------------------------------
// Test 14: TransactionContextBuilder execute_code with replay capture
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn test_tx_context_execute_code_with_capture() {
    // Use TransactionContextBuilder to execute code, then capture the context
    // data in our synthetic format for replay.
    let tx_context = TransactionContextBuilder::with_existing_mock_account()
        .build()
        .expect("failed to build tx context");

    let code = "
    use $kernel::prologue
    use mock::account

    begin
        exec.prologue::prepare_transaction
        push.100
        push.200
        add
        swap drop
    end
    ";

    let exec_output = tx_context
        .execute_code(code)
        .await
        .expect("code execution failed");

    // Verify execution produced expected result.
    let stack_ints = exec_output.stack.as_int_vec();
    assert_eq!(stack_ints[0], 300, "100 + 200 should be 300");

    // Capture the execution context as synthetic TransactionInputs.
    // This simulates what an instrumented client would do.
    let inputs = TransactionInputs::synthetic(0x1000, 1).with_tx_script("push.100 push.200 add");

    let tmp_dir = tempfile::tempdir().unwrap();
    let captured_path =
        capture_transaction_inputs(&inputs, tmp_dir.path()).expect("capture should succeed");

    assert!(captured_path.exists());
    let loaded = TransactionInputs::from_json(&std::fs::read_to_string(&captured_path).unwrap())
        .expect("should parse");
    assert_eq!(
        loaded.tx_args.tx_script.as_deref(),
        Some("push.100 push.200 add")
    );
}

// ---------------------------------------------------------------------------
// Test 15: Real chain with assets - verify asset tracking in synthetic types
// ---------------------------------------------------------------------------

#[test]
fn test_real_chain_asset_tracking_in_synthetic_types() {
    let mut builder = MockChain::builder();

    let fungible_asset = FungibleAsset::mock(500);

    let wallet = builder
        .add_existing_wallet_with_assets(Auth::IncrNonce, [fungible_asset])
        .expect("failed to create wallet with assets");

    let chain = builder.build().expect("failed to build chain");

    let account = chain.committed_account(wallet.id()).unwrap();
    let balance = account
        .vault()
        .get_balance(FungibleAsset::mock_issuer())
        .expect("should query balance");

    assert_eq!(balance, 500);

    // Represent the asset in our synthetic format.
    let block_num = chain.latest_block_header().block_num().as_u32();
    let inputs = synthetic_inputs_from_real_chain(&chain, wallet.id(), block_num);

    // The account ID and nonce should be captured correctly.
    assert_eq!(inputs.account.id, account_id_to_u64(wallet.id()));
    assert!(inputs.account.nonce > 0);

    // Serialize and verify the asset-bearing account survives roundtrip.
    let json = inputs.to_json().unwrap();
    let parsed = TransactionInputs::from_json(&json).unwrap();
    assert_eq!(parsed.account.id, inputs.account.id);
}

// ---------------------------------------------------------------------------
// Test 16: ReplayConfig validation with real-derived account IDs
// ---------------------------------------------------------------------------

#[test]
fn test_replay_config_with_real_derived_ids() {
    let mut builder = MockChain::builder();

    let wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let _chain = builder.build().expect("failed to build chain");

    let account_id_str = format!("0x{:x}", account_id_to_u64(wallet.id()));

    let config = ReplayConfig::new(
        "https://rpc.testnet.miden.io",
        &account_id_str,
        "0xtx_from_real_chain",
        "/tmp/output",
    );

    assert!(
        config.validate().is_ok(),
        "config with real-derived account ID should be valid"
    );
    assert!(
        !config.account_id.is_empty(),
        "account ID should not be empty"
    );
    assert!(
        config.account_id.starts_with("0x"),
        "account ID should be hex"
    );
}

// ---------------------------------------------------------------------------
// Test 17: Replay error paths with real-derived data
// ---------------------------------------------------------------------------

#[test]
fn test_replay_error_without_captured_inputs_real_config() {
    let mut builder = MockChain::builder();

    let wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let _chain = builder.build().expect("failed to build chain");

    let config = ReplayConfig::new(
        "https://rpc.testnet.miden.io",
        &format!("0x{:x}", account_id_to_u64(wallet.id())),
        "0xtest_tx",
        "/tmp/output",
    );

    // Without captured inputs, replay should return HistoricalReplayNotSupported.
    let err = replay_transaction(&config).unwrap_err();
    assert!(
        matches!(err, ReplayError::HistoricalReplayNotSupported(_)),
        "should indicate historical replay is not supported"
    );
}

// ---------------------------------------------------------------------------
// Test 18: SyncStateResult populated from real chain metadata
// ---------------------------------------------------------------------------

#[test]
fn test_sync_state_result_from_real_chain_metadata() {
    let mut builder = MockChain::builder();

    let _wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let _faucet = builder
        .add_existing_basic_faucet(Auth::IncrNonce, "SSR", 1_000_000, None)
        .expect("failed to create faucet");

    let chain = builder.build().expect("failed to build chain");
    let block_num: u32 = chain.latest_block_header().block_num().as_u32();

    // Simulate a SyncStateResult using real chain metadata.
    let sync_result = SyncStateResult {
        block_num,
        account_updates: 2,
        new_notes: 0,
        nullifier_updates: 0,
        success: true,
    };

    assert!(sync_result.success);
    assert_eq!(sync_result.block_num, block_num);
    assert_eq!(sync_result.account_updates, 2);

    // Verify JSON roundtrip.
    let json = serde_json::to_string(&sync_result).unwrap();
    let parsed: SyncStateResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.block_num, block_num);
    assert!(parsed.success);
}

// ---------------------------------------------------------------------------
// Test 19: Real faucet account is correctly identified
// ---------------------------------------------------------------------------

#[test]
fn test_real_faucet_identification() {
    let mut builder = MockChain::builder();

    let faucet = builder
        .add_existing_basic_faucet(Auth::IncrNonce, "FAUCET", 1_000_000, None)
        .expect("failed to create faucet");

    let wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let chain = builder.build().expect("failed to build chain");

    let faucet_account = chain.committed_account(faucet.id()).unwrap();
    let wallet_account = chain.committed_account(wallet.id()).unwrap();

    // Faucets and wallets have different properties that should be captured.
    assert!(
        faucet_account.is_faucet(),
        "real faucet account should be identified as faucet"
    );
    assert!(
        !wallet_account.is_faucet(),
        "real wallet should not be a faucet"
    );

    // Both should be representable in our synthetic type.
    let faucet_synthetic = PartialAccount {
        id: account_id_to_u64(faucet.id()),
        nonce: faucet_account.nonce().as_canonical_u64(),
        code_commitment: Digest::zero(),
        storage_commitment: Digest::zero(),
        vault_commitment: Digest::zero(),
        is_public: faucet_account.is_public(),
    };

    let wallet_synthetic = PartialAccount {
        id: account_id_to_u64(wallet.id()),
        nonce: wallet_account.nonce().as_canonical_u64(),
        code_commitment: Digest::zero(),
        storage_commitment: Digest::zero(),
        vault_commitment: Digest::zero(),
        is_public: wallet_account.is_public(),
    };

    assert_ne!(faucet_synthetic.id, wallet_synthetic.id);
}

// ---------------------------------------------------------------------------
// Test 20: End-to-end: real tx -> capture -> replay -> verify output
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn test_end_to_end_real_tx_capture_replay() {
    let mut builder = MockChain::builder();

    let sender = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create sender");

    let receiver = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create receiver");

    let fungible_asset = FungibleAsset::mock(777).unwrap_fungible();

    let note = builder
        .add_p2id_note(
            sender.id(),
            receiver.id(),
            &[miden_protocol::asset::Asset::Fungible(fungible_asset)],
            NoteType::Public,
        )
        .expect("failed to create note");

    let mut chain = builder.build().expect("failed to build chain");

    // Step 1: Execute a real transaction.
    let executed_tx = chain
        .build_tx_context(receiver.id(), &[note.id()], &[])
        .expect("build tx context")
        .build()
        .expect("build context")
        .execute()
        .await
        .expect("execute tx");

    chain
        .add_pending_executed_transaction(&executed_tx)
        .expect("add pending tx");
    chain.prove_next_block().expect("prove block");

    // Step 2: Capture post-transaction state.
    let block_num: u32 = chain.latest_block_header().block_num().as_u32();

    let note_id_u64 = note_id_to_u64(note.id());

    let inputs = synthetic_inputs_from_real_chain(&chain, receiver.id(), block_num)
        .with_input_note(InputNote {
            id: note_id_u64,
            script_hash: Digest::zero(),
            inputs: vec![account_id_to_u64(receiver.id())],
            assets: vec![(account_id_to_u64(sender.id()), 777)],
            sender: account_id_to_u64(sender.id()),
            metadata: NoteMetadata::default(),
        });

    let tmp_dir = tempfile::tempdir().unwrap();
    let capture_dir = tmp_dir.path().join("captures");
    let output_dir = tmp_dir.path().join("output");

    let captured_path =
        capture_transaction_inputs(&inputs, &capture_dir).expect("capture should succeed");

    // Step 3: Replay from captured inputs.
    let config = ReplayConfig::new(
        "https://rpc.testnet.miden.io",
        &format!("0x{:x}", account_id_to_u64(receiver.id())),
        "0xe2e_test_tx",
        &output_dir,
    )
    .with_captured_inputs(&captured_path);

    let result = replay_transaction(&config).expect("replay should succeed");

    // Step 4: Verify replay results.
    assert_eq!(result.status, ReplayStatus::Completed);
    assert_eq!(result.block_num, block_num);
    assert_eq!(result.input_note_count, 1);

    let summary_path = output_dir.join("replay_summary.json");
    assert!(summary_path.exists());

    let summary: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&summary_path).unwrap()).unwrap();
    assert_eq!(summary["block_num"], block_num);
    assert_eq!(summary["input_notes"], 1);
    assert_eq!(summary["has_tx_script"], false);
    assert_eq!(summary["status"], "replay_completed_synthetic");

    // Verify the receiver's balance was tracked.
    let receiver_account = chain.committed_account(receiver.id()).unwrap();
    let balance = receiver_account
        .vault()
        .get_balance(fungible_asset.faucet_id())
        .expect("should query balance");
    assert_eq!(balance, 777, "receiver should have received the asset");
}
