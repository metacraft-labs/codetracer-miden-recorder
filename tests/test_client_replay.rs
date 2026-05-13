//! Tests for on-chain transaction replay via miden-client (M6).
//!
//! These tests validate the client replay infrastructure, DataStore
//! implementation, TransactionInputs construction, state sync simulation,
//! and the full replay pipeline using synthetic data.
//!
//! When miden-client becomes available at a version compatible with our
//! miden-processor dependency, these tests should be supplemented with
//! integration tests that sync from a real testnet node.

use std::path::PathBuf;

use codetracer_miden_recorder::client_replay::*;

// ---------------------------------------------------------------------------
// Test 1: ReplayConfig construction and validation
// ---------------------------------------------------------------------------

#[test]
fn test_replay_config_construction() {
    let config = ReplayConfig::new(
        "https://rpc.testnet.miden.io",
        "0xabc123",
        "0xdef456",
        "/tmp/ct-replay-output",
    );

    assert_eq!(config.node_url, "https://rpc.testnet.miden.io");
    assert_eq!(config.account_id, "0xabc123");
    assert_eq!(config.transaction_id, "0xdef456");
    assert_eq!(config.output_dir, PathBuf::from("/tmp/ct-replay-output"));
    assert!(config.captured_inputs_path.is_none());
    assert!(config.validate().is_ok());
}

#[test]
fn test_replay_config_with_captured_inputs() {
    let config = ReplayConfig::new(
        "https://rpc.testnet.miden.io",
        "0xabc123",
        "0xdef456",
        "/tmp/ct-replay-output",
    )
    .with_captured_inputs("/path/to/captured_inputs.json");

    assert_eq!(
        config.captured_inputs_path,
        Some(PathBuf::from("/path/to/captured_inputs.json"))
    );
}

#[test]
fn test_replay_config_validation_empty_node_url() {
    let config = ReplayConfig::new("", "0xabc123", "0xdef456", "/tmp/out");
    let err = config.validate().unwrap_err();
    assert!(matches!(err, ReplayError::InvalidConfig(_)));
    assert!(err.to_string().contains("node_url"));
}

#[test]
fn test_replay_config_validation_empty_account_id() {
    let config = ReplayConfig::new("http://node", "", "0xdef456", "/tmp/out");
    let err = config.validate().unwrap_err();
    assert!(matches!(err, ReplayError::InvalidConfig(_)));
    assert!(err.to_string().contains("account_id"));
}

#[test]
fn test_replay_config_validation_empty_transaction_id() {
    let config = ReplayConfig::new("http://node", "0xabc", "", "/tmp/out");
    let err = config.validate().unwrap_err();
    assert!(matches!(err, ReplayError::InvalidConfig(_)));
    assert!(err.to_string().contains("transaction_id"));
}

// ---------------------------------------------------------------------------
// Test 2: TransactionInputs construction
// ---------------------------------------------------------------------------

#[test]
fn test_transaction_inputs_construction() {
    let inputs = TransactionInputs::synthetic(0x1000, 42);

    assert_eq!(inputs.account.id, 0x1000);
    assert_eq!(inputs.account.nonce, 1);
    assert!(inputs.account.is_public);
    assert_eq!(inputs.block_header.block_num, 42);
    assert_eq!(inputs.block_header.version, 1);
    assert!(inputs.input_notes.notes.is_empty());
    assert!(inputs.tx_args.tx_script.is_none());
}

#[test]
fn test_transaction_inputs_with_note() {
    let note = InputNote {
        id: 1,
        script_hash: Digest::zero(),
        inputs: vec![10, 20, 30],
        assets: vec![(0x2000, 100)],
        sender: 0x2000,
        metadata: NoteMetadata::default(),
    };

    let inputs = TransactionInputs::synthetic(0x1000, 42).with_input_note(note);

    assert_eq!(inputs.input_notes.notes.len(), 1);
    assert_eq!(inputs.input_notes.notes[0].id, 1);
    assert_eq!(inputs.input_notes.notes[0].inputs, vec![10, 20, 30]);
    assert_eq!(inputs.input_notes.notes[0].assets, vec![(0x2000, 100)]);
}

#[test]
fn test_transaction_inputs_with_tx_script() {
    let inputs =
        TransactionInputs::synthetic(0x1000, 42).with_tx_script("begin push.1 push.2 add end");

    assert_eq!(
        inputs.tx_args.tx_script.as_deref(),
        Some("begin push.1 push.2 add end")
    );
}

#[test]
fn test_transaction_inputs_json_roundtrip() {
    let note = InputNote {
        id: 42,
        script_hash: Digest::zero(),
        inputs: vec![100, 200],
        assets: vec![(0x2000, 500)],
        sender: 0x3000,
        metadata: NoteMetadata {
            tag: 1,
            aux: 99,
            note_type: 0,
        },
    };

    let inputs = TransactionInputs::synthetic(0xABCD, 100)
        .with_input_note(note)
        .with_tx_script("begin push.42 end");

    let json = inputs.to_json().unwrap();
    let parsed = TransactionInputs::from_json(&json).unwrap();

    assert_eq!(parsed.account.id, 0xABCD);
    assert_eq!(parsed.block_header.block_num, 100);
    assert_eq!(parsed.input_notes.notes.len(), 1);
    assert_eq!(parsed.input_notes.notes[0].id, 42);
    assert_eq!(parsed.input_notes.notes[0].inputs, vec![100, 200]);
    assert_eq!(
        parsed.tx_args.tx_script.as_deref(),
        Some("begin push.42 end")
    );
}

#[test]
fn test_transaction_inputs_json_parse_error() {
    let err = TransactionInputs::from_json("not valid json").unwrap_err();
    assert!(matches!(err, ReplayError::CapturedInputsLoadFailed(_)));
}

// ---------------------------------------------------------------------------
// Test 3: ReplayDataStore
// ---------------------------------------------------------------------------

#[test]
fn test_replay_data_store_empty() {
    let store = ReplayDataStore::new();
    assert_eq!(store.account_count(), 0);
    assert_eq!(store.note_count(), 0);
    assert_eq!(store.block_header_count(), 0);
    assert_eq!(store.latest_block(), 0);
}

#[test]
fn test_replay_data_store_insert_and_query() {
    let mut store = ReplayDataStore::new();

    let account = PartialAccount {
        id: 0x1000,
        nonce: 5,
        code_commitment: Digest::zero(),
        storage_commitment: Digest::zero(),
        vault_commitment: Digest::zero(),
        is_public: true,
    };
    store.insert_account(account);

    let header = BlockHeader::synthetic(42);
    store.insert_block_header(header);

    let note = InputNote {
        id: 1,
        script_hash: Digest::zero(),
        inputs: vec![10],
        assets: vec![],
        sender: 0x2000,
        metadata: NoteMetadata::default(),
    };
    store.insert_note(note);

    assert_eq!(store.account_count(), 1);
    assert_eq!(store.note_count(), 1);
    assert_eq!(store.block_header_count(), 1);
    assert_eq!(store.latest_block(), 42);
    assert!(store.has_account(0x1000));
    assert!(!store.has_account(0x9999));
    assert!(store.has_note(1));
    assert!(!store.has_note(999));
}

#[test]
fn test_replay_data_store_get_transaction_inputs() {
    let mut store = ReplayDataStore::new();

    store.insert_account(PartialAccount {
        id: 0x1000,
        nonce: 3,
        code_commitment: Digest::zero(),
        storage_commitment: Digest::zero(),
        vault_commitment: Digest::zero(),
        is_public: true,
    });
    store.insert_block_header(BlockHeader::synthetic(50));
    store.insert_note(InputNote {
        id: 1,
        script_hash: Digest::zero(),
        inputs: vec![42],
        assets: vec![(0x2000, 100)],
        sender: 0x2000,
        metadata: NoteMetadata::default(),
    });
    store.insert_note(InputNote {
        id: 2,
        script_hash: Digest::zero(),
        inputs: vec![99],
        assets: vec![],
        sender: 0x3000,
        metadata: NoteMetadata::default(),
    });

    let inputs = store.get_transaction_inputs(0x1000, 50, &[1, 2]).unwrap();

    assert_eq!(inputs.account.id, 0x1000);
    assert_eq!(inputs.account.nonce, 3);
    assert_eq!(inputs.block_header.block_num, 50);
    assert_eq!(inputs.input_notes.notes.len(), 2);
    assert_eq!(inputs.input_notes.notes[0].id, 1);
    assert_eq!(inputs.input_notes.notes[1].id, 2);
}

#[test]
fn test_replay_data_store_account_not_found() {
    let store = ReplayDataStore::new();
    let err = store.get_transaction_inputs(0x9999, 0, &[]).unwrap_err();
    assert!(matches!(err, ReplayError::AccountNotFound(_)));
}

#[test]
fn test_replay_data_store_note_not_found() {
    let mut store = ReplayDataStore::new();
    store.insert_account(PartialAccount {
        id: 0x1000,
        nonce: 1,
        code_commitment: Digest::zero(),
        storage_commitment: Digest::zero(),
        vault_commitment: Digest::zero(),
        is_public: true,
    });

    let err = store.get_transaction_inputs(0x1000, 0, &[999]).unwrap_err();
    assert!(matches!(err, ReplayError::InputsNotAvailable(_)));
}

#[test]
fn test_replay_data_store_from_captured_inputs() {
    let note = InputNote {
        id: 7,
        script_hash: Digest::zero(),
        inputs: vec![1, 2, 3],
        assets: vec![(0x2000, 50)],
        sender: 0x2000,
        metadata: NoteMetadata::default(),
    };
    let inputs = TransactionInputs::synthetic(0xABC, 100).with_input_note(note);

    let store = ReplayDataStore::from_captured_inputs(&inputs);

    assert_eq!(store.account_count(), 1);
    assert!(store.has_account(0xABC));
    assert_eq!(store.latest_block(), 100);
    assert!(store.has_note(7));
    assert_eq!(store.block_header_count(), 1);
}

// ---------------------------------------------------------------------------
// Test 4: Sync state updates store
// ---------------------------------------------------------------------------

#[test]
fn test_sync_state_updates_store() {
    let mut client = MidenNodeClient::new("https://rpc.testnet.miden.io");
    assert!(!client.is_connected());
    assert_eq!(client.node_url(), "https://rpc.testnet.miden.io");

    // Sync with synthetic data.
    let accounts = vec![
        PartialAccount {
            id: 0x1000,
            nonce: 1,
            code_commitment: Digest::zero(),
            storage_commitment: Digest::zero(),
            vault_commitment: Digest::zero(),
            is_public: true,
        },
        PartialAccount {
            id: 0x2000,
            nonce: 0,
            code_commitment: Digest::zero(),
            storage_commitment: Digest::zero(),
            vault_commitment: Digest::zero(),
            is_public: false,
        },
    ];

    let notes = vec![InputNote {
        id: 1,
        script_hash: Digest::zero(),
        inputs: vec![42],
        assets: vec![(0x2000, 100)],
        sender: 0x2000,
        metadata: NoteMetadata::default(),
    }];

    let headers = vec![
        BlockHeader::synthetic(10),
        BlockHeader::synthetic(11),
        BlockHeader::synthetic(12),
    ];

    let result = client.sync_state_synthetic(accounts, notes, headers);

    assert!(result.success);
    assert_eq!(result.block_num, 12);
    assert_eq!(result.account_updates, 2);
    assert_eq!(result.new_notes, 1);
    assert!(client.is_connected());

    // Verify the store was populated.
    let store = client.store();
    assert_eq!(store.account_count(), 2);
    assert_eq!(store.note_count(), 1);
    assert_eq!(store.block_header_count(), 3);
    assert!(store.has_account(0x1000));
    assert!(store.has_account(0x2000));
    assert!(store.has_note(1));
}

#[test]
fn test_sync_state_real_node_not_available() {
    let mut client = MidenNodeClient::new("https://rpc.testnet.miden.io");
    let err = client.sync_state().unwrap_err();
    assert!(matches!(err, ReplayError::NodeConnectionFailed(_)));
    assert!(err.to_string().contains("miden-client is not available"));
}

// ---------------------------------------------------------------------------
// Test 5: Replay pipeline with synthetic data (end-to-end)
// ---------------------------------------------------------------------------

#[test]
fn test_replay_pipeline_synthetic() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let capture_dir = tmp_dir.path().join("captures");
    let output_dir = tmp_dir.path().join("output");

    // Step 1: Create synthetic TransactionInputs.
    let note = InputNote {
        id: 1,
        script_hash: Digest::zero(),
        inputs: vec![10, 20, 30],
        assets: vec![(0x2000, 100)],
        sender: 0x2000,
        metadata: NoteMetadata::default(),
    };
    let inputs = TransactionInputs::synthetic(0x1000, 42)
        .with_input_note(note)
        .with_tx_script("begin push.1 end");

    // Step 2: Capture inputs to disk.
    let captured_path = capture_transaction_inputs(&inputs, &capture_dir).unwrap();
    assert!(captured_path.exists());
    assert!(
        captured_path
            .to_string_lossy()
            .contains("tx_inputs_0x1000_block_42.json")
    );

    // Step 3: Verify we can load the captured file.
    let loaded_json = std::fs::read_to_string(&captured_path).unwrap();
    let loaded = TransactionInputs::from_json(&loaded_json).unwrap();
    assert_eq!(loaded.account.id, 0x1000);
    assert_eq!(loaded.block_header.block_num, 42);
    assert_eq!(loaded.input_notes.notes.len(), 1);

    // Step 4: Replay using captured inputs.
    let config = ReplayConfig::new(
        "https://rpc.testnet.miden.io",
        "0x1000",
        "0xdeadbeef",
        &output_dir,
    )
    .with_captured_inputs(&captured_path);

    let result = replay_transaction(&config).unwrap();

    assert_eq!(result.status, ReplayStatus::Completed);
    assert_eq!(result.block_num, 42);
    assert_eq!(result.input_note_count, 1);
    assert_eq!(result.transaction_id, "0xdeadbeef");

    // Step 5: Verify output files were created.
    let summary_path = output_dir.join("replay_summary.json");
    assert!(summary_path.exists());

    let summary_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&summary_path).unwrap()).unwrap();
    assert_eq!(summary_json["account_id"], "0x1000");
    assert_eq!(summary_json["block_num"], 42);
    assert_eq!(summary_json["input_notes"], 1);
    assert_eq!(summary_json["has_tx_script"], true);
}

// ---------------------------------------------------------------------------
// Test 6: Capture transaction inputs
// ---------------------------------------------------------------------------

#[test]
fn test_capture_transaction_inputs() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let capture_dir = tmp_dir.path().join("captures");

    let inputs = TransactionInputs::synthetic(0xABCD, 77);
    let path = capture_transaction_inputs(&inputs, &capture_dir).unwrap();

    assert!(path.exists());
    assert!(
        path.to_string_lossy()
            .contains("tx_inputs_0xabcd_block_77.json")
    );

    // Verify the file content is valid JSON that roundtrips.
    let json = std::fs::read_to_string(&path).unwrap();
    let loaded = TransactionInputs::from_json(&json).unwrap();
    assert_eq!(loaded.account.id, 0xABCD);
    assert_eq!(loaded.block_header.block_num, 77);
}

#[test]
fn test_capture_transaction_inputs_creates_directory() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let nested_dir = tmp_dir.path().join("a").join("b").join("c");

    let inputs = TransactionInputs::synthetic(0x1, 1);
    let path = capture_transaction_inputs(&inputs, &nested_dir).unwrap();
    assert!(path.exists());
}

#[test]
fn test_capture_multiple_transactions() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let capture_dir = tmp_dir.path().join("captures");

    let inputs1 = TransactionInputs::synthetic(0x1000, 10);
    let inputs2 = TransactionInputs::synthetic(0x2000, 20);
    let inputs3 = TransactionInputs::synthetic(0x1000, 30); // same account, different block

    let path1 = capture_transaction_inputs(&inputs1, &capture_dir).unwrap();
    let path2 = capture_transaction_inputs(&inputs2, &capture_dir).unwrap();
    let path3 = capture_transaction_inputs(&inputs3, &capture_dir).unwrap();

    assert_ne!(path1, path2);
    assert_ne!(path1, path3);
    assert!(path1.exists());
    assert!(path2.exists());
    assert!(path3.exists());
}

// ---------------------------------------------------------------------------
// Test 7: Replay limitations documented (error messages)
// ---------------------------------------------------------------------------

#[test]
fn test_replay_limitations_documented() {
    // Without captured inputs, replay should explain the limitation.
    let config = ReplayConfig::new(
        "https://rpc.testnet.miden.io",
        "0x1000",
        "0xdeadbeef",
        "/tmp/output",
    );

    let err = replay_transaction(&config).unwrap_err();
    assert!(matches!(err, ReplayError::HistoricalReplayNotSupported(_)));

    let msg = err.to_string();
    assert!(
        msg.contains("Historical transaction replay"),
        "Error should mention historical replay limitation"
    );
    assert!(
        msg.contains("capture_transaction_inputs"),
        "Error should suggest the capture workflow"
    );
    assert!(
        msg.contains("miden-client"),
        "Error should mention miden-client dependency"
    );
}

#[test]
fn test_replay_node_sync_limitation() {
    let mut client = MidenNodeClient::new("http://localhost:57291");
    let err = client.sync_state().unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("miden-client is not available"));
    assert!(msg.contains("captured TransactionInputs"));
}

#[test]
fn test_replay_error_variants() {
    // Verify all error variants produce meaningful messages.
    let errors = vec![
        ReplayError::InvalidConfig("test".into()),
        ReplayError::NodeConnectionFailed("test".into()),
        ReplayError::SyncFailed("test".into()),
        ReplayError::AccountNotFound("test".into()),
        ReplayError::InputsNotAvailable("test".into()),
        ReplayError::ExecutionFailed("test".into()),
        ReplayError::TraceOutputFailed("test".into()),
        ReplayError::HistoricalReplayNotSupported("test".into()),
        ReplayError::ForeignAccountNotFound("test".into()),
        ReplayError::CapturedInputsLoadFailed("test".into()),
        ReplayError::IoError("test".into()),
    ];

    for err in &errors {
        let msg = err.to_string();
        assert!(!msg.is_empty(), "Error message should not be empty");
        assert!(
            msg.contains("test"),
            "Error message should contain inner msg"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 8: Foreign account lookup
// ---------------------------------------------------------------------------

#[test]
fn test_foreign_account_lookup() {
    let mut store = ReplayDataStore::new();

    // Insert a foreign account.
    let foreign = ForeignAccountData {
        id: 0x5000,
        account_hash: Digest::zero(),
        code_commitment: Digest::zero(),
        storage_commitment: Digest::zero(),
        witness: vec![Digest::zero(), Digest::zero()],
    };
    store.insert_foreign_account(foreign);

    // Look it up.
    let result = store.get_foreign_account_inputs(0x5000).unwrap();
    assert_eq!(result.id, 0x5000);
    assert_eq!(result.witness.len(), 2);
}

#[test]
fn test_foreign_account_not_found() {
    let store = ReplayDataStore::new();
    let err = store.get_foreign_account_inputs(0x9999).unwrap_err();
    assert!(matches!(err, ReplayError::ForeignAccountNotFound(_)));
}

#[test]
fn test_foreign_account_via_client() {
    let mut client = MidenNodeClient::new("http://node");

    // Insert foreign account data into the client's store.
    client
        .store_mut()
        .insert_foreign_account(ForeignAccountData {
            id: 0x7777,
            account_hash: Digest::zero(),
            code_commitment: Digest::zero(),
            storage_commitment: Digest::zero(),
            witness: vec![Digest::zero()],
        });

    let result = client.get_foreign_account_inputs(0x7777).unwrap();
    assert_eq!(result.id, 0x7777);

    let err = client.get_foreign_account_inputs(0x8888).unwrap_err();
    assert!(matches!(err, ReplayError::ForeignAccountNotFound(_)));
}

// ---------------------------------------------------------------------------
// Test: Block header synthetic construction
// ---------------------------------------------------------------------------

#[test]
fn test_block_header_synthetic() {
    let header = BlockHeader::synthetic(100);
    assert_eq!(header.block_num, 100);
    assert_eq!(header.version, 1);
    assert_eq!(header.prev_hash, Digest::zero());
    // Timestamp should be deterministic based on block_num.
    assert_eq!(header.timestamp, 1700000000 + 100 * 10);
}

// ---------------------------------------------------------------------------
// Test: DataStore trait object usage
// ---------------------------------------------------------------------------

#[test]
fn test_data_store_trait_object() {
    let mut store = ReplayDataStore::new();
    store.insert_account(PartialAccount {
        id: 0x1000,
        nonce: 1,
        code_commitment: Digest::zero(),
        storage_commitment: Digest::zero(),
        vault_commitment: Digest::zero(),
        is_public: true,
    });
    store.insert_block_header(BlockHeader::synthetic(50));

    // Use as trait object to verify the trait is object-safe.
    let ds: &dyn DataStore = &store;
    let inputs = ds.get_transaction_inputs(0x1000, 50, &[]).unwrap();
    assert_eq!(inputs.account.id, 0x1000);
    assert_eq!(inputs.block_header.block_num, 50);
}

// ---------------------------------------------------------------------------
// Test: ReplayConfig serialization
// ---------------------------------------------------------------------------

#[test]
fn test_replay_config_serialization() {
    let config = ReplayConfig::new(
        "https://rpc.testnet.miden.io",
        "0x1000",
        "0xdeadbeef",
        "/tmp/output",
    )
    .with_captured_inputs("/path/to/inputs.json");

    let json = serde_json::to_string(&config).unwrap();
    let parsed: ReplayConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.node_url, config.node_url);
    assert_eq!(parsed.account_id, config.account_id);
    assert_eq!(parsed.transaction_id, config.transaction_id);
    assert_eq!(parsed.output_dir, config.output_dir);
    assert_eq!(parsed.captured_inputs_path, config.captured_inputs_path);
}
