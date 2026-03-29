//! Tests for MockChain contract-level testing support (M5).
//!
//! These tests validate the MockChain infrastructure, execution context
//! tracking, kernel procedure detection, and the contract trace session
//! lifecycle using synthetic data.
//!
//! When miden-testing becomes available at a version compatible with our
//! miden-processor dependency, these tests should be supplemented with
//! integration tests that use the real MockChain and TransactionExecutor.

use codetracer_miden_recorder::kernel_procs::{
    is_kernel_procedure, kernel_proc_info, procedure_display_name,
};
use codetracer_miden_recorder::mockchain::*;

// ---------------------------------------------------------------------------
// Test 1: MockChainBuilder creates a valid chain
// ---------------------------------------------------------------------------

#[test]
fn test_mockchain_builder_creates_chain() {
    let wallet_id = AccountId(0x1000);
    let faucet_id = AccountId(0x2000);

    let chain = MockChainBuilder::new()
        .add_existing_wallet(WalletConfig {
            id: wallet_id,
            initial_assets: AssetVault::default(),
        })
        .add_existing_basic_faucet(FaucetConfig {
            id: faucet_id,
            symbol: "TEST".to_string(),
            max_supply: 1_000_000,
            initial_supply: 0,
        })
        .add_p2id_note(P2IdNoteConfig {
            id: NoteId(1),
            sender: faucet_id,
            receiver: wallet_id,
            assets: AssetVault {
                fungible: vec![FungibleAsset {
                    faucet_id,
                    amount: 100,
                }],
            },
            note_type: NoteType::Public,
            aux: 0,
        })
        .build();

    // Verify accounts were created.
    assert_eq!(chain.account_count(), 2, "should have wallet + faucet");
    assert!(chain.has_account(&wallet_id), "wallet should exist");
    assert!(chain.has_account(&faucet_id), "faucet should exist");

    // Verify the note was created.
    assert_eq!(chain.note_count(), 1, "should have one note");
    let note = chain.get_note(&NoteId(1)).expect("note should exist");
    assert_eq!(note.sender, faucet_id);
    assert!(!note.consumed, "note should not be consumed yet");
    assert_eq!(note.note_type, NoteType::Public);

    // Verify asset in the note.
    assert_eq!(note.assets.fungible.len(), 1);
    assert_eq!(note.assets.fungible[0].amount, 100);
    assert_eq!(note.assets.fungible[0].faucet_id, faucet_id);
}

// ---------------------------------------------------------------------------
// Test 2: Wallet and faucet setup
// ---------------------------------------------------------------------------

#[test]
fn test_wallet_and_faucet_setup() {
    let wallet_id = AccountId(0x1000);
    let faucet_id = AccountId(0x2000);

    let wallet_with_assets = WalletConfig {
        id: wallet_id,
        initial_assets: AssetVault {
            fungible: vec![FungibleAsset {
                faucet_id,
                amount: 500,
            }],
        },
    };

    let faucet = FaucetConfig {
        id: faucet_id,
        symbol: "GOLD".to_string(),
        max_supply: 10_000_000,
        initial_supply: 1_000,
    };

    let chain = MockChainBuilder::new()
        .add_existing_wallet(wallet_with_assets)
        .add_existing_basic_faucet(faucet)
        .build();

    // Check wallet.
    let wallet = chain.get_account(&wallet_id).expect("wallet should exist");
    assert!(
        matches!(wallet.kind, AccountKind::Wallet),
        "should be a wallet"
    );
    assert_eq!(wallet.assets.fungible.len(), 1);
    assert_eq!(wallet.assets.fungible[0].amount, 500);

    // Check faucet.
    let faucet_account = chain.get_account(&faucet_id).expect("faucet should exist");
    match &faucet_account.kind {
        AccountKind::Faucet {
            symbol,
            max_supply,
            current_supply,
        } => {
            assert_eq!(symbol, "GOLD");
            assert_eq!(*max_supply, 10_000_000);
            assert_eq!(*current_supply, 1_000);
        }
        _ => panic!("expected faucet account kind"),
    }
}

// ---------------------------------------------------------------------------
// Test 3: Transaction context construction
// ---------------------------------------------------------------------------

#[test]
fn test_transaction_context_construction() {
    let wallet_id = AccountId(0x1000);
    let faucet_id = AccountId(0x2000);
    let note_id = NoteId(42);

    let config = MockChainConfig {
        wallets: vec![WalletConfig {
            id: wallet_id,
            initial_assets: AssetVault::default(),
        }],
        faucets: vec![FaucetConfig {
            id: faucet_id,
            symbol: "TEST".to_string(),
            max_supply: 1_000_000,
            initial_supply: 0,
        }],
        p2id_notes: vec![P2IdNoteConfig {
            id: note_id,
            sender: faucet_id,
            receiver: wallet_id,
            assets: AssetVault {
                fungible: vec![FungibleAsset {
                    faucet_id,
                    amount: 200,
                }],
            },
            note_type: NoteType::Public,
            aux: 0,
        }],
        ..Default::default()
    };

    let mut session = ContractTraceSession::new(config, std::env::temp_dir().join("test_tx_ctx"));
    assert_eq!(*session.status(), SessionStatus::Created);

    // Build the chain.
    session.build_chain().expect("build_chain should succeed");
    assert_eq!(*session.status(), SessionStatus::ChainBuilt);

    // Verify chain state.
    let chain = session.chain().expect("chain should be built");
    assert!(chain.has_account(&wallet_id));
    assert!(chain.has_account(&faucet_id));
    assert!(chain.get_note(&note_id).is_some());

    // Construct transaction config.
    let tx_config = TransactionConfig {
        account_id: wallet_id,
        input_notes: vec![note_id],
        tx_script: None,
        debug_mode: true,
    };

    // Verify the tx config is valid against the chain.
    assert!(chain.has_account(&tx_config.account_id));
    for nid in &tx_config.input_notes {
        assert!(chain.get_note(nid).is_some());
    }
}

// ---------------------------------------------------------------------------
// Test 4: Execution context tracking
// ---------------------------------------------------------------------------

#[test]
fn test_execution_context_tracking() {
    let mut tracker = ExecutionContextTracker::new();

    // Initially no context.
    assert!(tracker.current().is_none());
    assert_eq!(tracker.context_count(), 0);

    // Switch to kernel context (context 0).
    tracker.switch_context(ContextId(0), ContextKind::Kernel, 0);
    assert_eq!(tracker.current(), Some(ContextId(0)));
    assert_eq!(tracker.context_count(), 1);
    assert!(tracker.is_in_kernel());

    // Switch to note script context (context 1).
    let note_kind = ContextKind::NoteScript {
        note_id: NoteId(1),
    };
    tracker.switch_context(ContextId(1), note_kind.clone(), 100);
    assert_eq!(tracker.current(), Some(ContextId(1)));
    assert_eq!(tracker.context_count(), 2);
    assert!(!tracker.is_in_kernel());

    // Switch back to kernel (syscall from note script).
    tracker.switch_context(ContextId(0), ContextKind::Kernel, 150);
    assert_eq!(tracker.current(), Some(ContextId(0)));
    assert!(tracker.is_in_kernel());
    // Context count should still be 2 (same context ID).
    assert_eq!(tracker.context_count(), 2);

    // Switch to account code context (context 2).
    let account_kind = ContextKind::AccountCode {
        account_id: AccountId(0x1000),
    };
    tracker.switch_context(ContextId(2), account_kind, 200);
    assert_eq!(tracker.context_count(), 3);

    // Verify switch history.
    let history = tracker.switch_history();
    assert_eq!(history.len(), 4);
    assert_eq!(history[0], (ContextId(0), 0));
    assert_eq!(history[1], (ContextId(1), 100));
    assert_eq!(history[2], (ContextId(0), 150));
    assert_eq!(history[3], (ContextId(2), 200));

    // Verify individual context info.
    let kernel_info = tracker.get_context(&ContextId(0)).unwrap();
    assert_eq!(kernel_info.kind, ContextKind::Kernel);
    assert!(kernel_info.entered);

    let note_info = tracker.get_context(&ContextId(1)).unwrap();
    assert!(matches!(note_info.kind, ContextKind::NoteScript { .. }));

    // All contexts.
    let all = tracker.all_contexts();
    assert_eq!(all.len(), 3);
}

// ---------------------------------------------------------------------------
// Test 5: Kernel procedure detection
// ---------------------------------------------------------------------------

#[test]
fn test_kernel_procedure_detection() {
    // Known kernel procedures.
    assert!(is_kernel_procedure("miden::kernel::account_vault_add_asset"));
    assert!(is_kernel_procedure("miden::kernel::get_account_id"));
    assert!(is_kernel_procedure("miden::note::get_inputs"));
    assert!(is_kernel_procedure("miden::tx::get_block_number"));
    assert!(is_kernel_procedure("miden::asset::build_fungible_asset"));
    assert!(is_kernel_procedure("miden::faucet::mint"));
    assert!(is_kernel_procedure("#sys::prologue"));
    assert!(is_kernel_procedure("#sys::epilogue"));

    // User procedures should not be detected as kernel.
    assert!(!is_kernel_procedure("my_contract::transfer"));
    assert!(!is_kernel_procedure("compute"));
    assert!(!is_kernel_procedure("#exec::compute"));
    assert!(!is_kernel_procedure(""));

    // Verify kernel proc info extraction.
    let info = kernel_proc_info("miden::kernel::account_vault_add_asset").unwrap();
    assert_eq!(info.short_name, "account_vault_add_asset");
    assert!(!info.is_prologue);

    let prologue_info = kernel_proc_info("#sys::prologue").unwrap();
    assert!(prologue_info.is_prologue);

    let epilogue_info = kernel_proc_info("#sys::epilogue").unwrap();
    assert!(epilogue_info.is_prologue); // epilogue is also a setup procedure

    assert!(kernel_proc_info("my_contract::foo").is_none());

    // Verify display names.
    assert_eq!(
        procedure_display_name("miden::kernel::get_account_id"),
        "[kernel] get_account_id"
    );
    assert_eq!(
        procedure_display_name("#sys::prologue"),
        "[kernel:setup] prologue"
    );
    assert_eq!(
        procedure_display_name("my_contract::transfer"),
        "my_contract::transfer"
    );

    // Context classification from procedure name.
    let kind = ExecutionContextTracker::classify_from_procedure("miden::kernel::get_account_id");
    assert_eq!(kind, ContextKind::Kernel);

    let kind = ExecutionContextTracker::classify_from_procedure("my_contract::transfer");
    assert_eq!(kind, ContextKind::Unknown);
}

// ---------------------------------------------------------------------------
// Test 6: Contract trace session lifecycle
// ---------------------------------------------------------------------------

#[test]
fn test_contract_trace_session_lifecycle() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("contract_trace");

    let wallet_id = AccountId(0x1000);
    let faucet_id = AccountId(0x2000);
    let note_id = NoteId(1);

    let config = MockChainConfig {
        wallets: vec![WalletConfig {
            id: wallet_id,
            initial_assets: AssetVault::default(),
        }],
        faucets: vec![FaucetConfig {
            id: faucet_id,
            symbol: "TEST".to_string(),
            max_supply: 1_000_000,
            initial_supply: 0,
        }],
        p2id_notes: vec![P2IdNoteConfig {
            id: note_id,
            sender: faucet_id,
            receiver: wallet_id,
            assets: AssetVault {
                fungible: vec![FungibleAsset {
                    faucet_id,
                    amount: 100,
                }],
            },
            note_type: NoteType::Public,
            aux: 0,
        }],
        ..Default::default()
    };

    let mut session = ContractTraceSession::new(config, out_dir.clone());

    // Step 1: Created state.
    assert_eq!(*session.status(), SessionStatus::Created);

    // Cannot execute before building chain.
    let tx = TransactionConfig {
        account_id: wallet_id,
        input_notes: vec![note_id],
        tx_script: None,
        debug_mode: true,
    };
    assert!(
        session.execute_transaction(&tx).is_err(),
        "should not execute before building chain"
    );

    // Step 2: Build chain.
    session.build_chain().expect("build_chain should succeed");
    assert_eq!(*session.status(), SessionStatus::ChainBuilt);

    // Cannot build chain twice.
    assert!(
        session.build_chain().is_err(),
        "should not build chain twice"
    );

    // Step 3: Execute transaction.
    session
        .execute_transaction(&tx)
        .expect("execute should succeed");
    assert_eq!(*session.status(), SessionStatus::Executed);

    // Verify trace events were generated.
    let events = session.trace_events();
    assert!(!events.is_empty(), "should have trace events");

    // Verify we have context switches.
    let context_switches: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::ContextSwitch { .. }))
        .collect();
    assert!(
        !context_switches.is_empty(),
        "should have context switches"
    );

    // Verify we have kernel calls.
    let kernel_calls: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::Call { is_kernel: true, .. }))
        .collect();
    assert!(!kernel_calls.is_empty(), "should have kernel calls");

    // Verify we have user calls.
    let user_calls: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::Call { is_kernel: false, .. }))
        .collect();
    assert!(!user_calls.is_empty(), "should have user calls");

    // Verify context tracker.
    let tracker = session.context_tracker();
    assert!(tracker.context_count() >= 2, "should have at least kernel + note contexts");

    // Step 4: Finalize.
    let summary_path = session.finalize().expect("finalize should succeed");
    assert_eq!(*session.status(), SessionStatus::Completed);
    assert!(summary_path.exists(), "summary file should exist");

    // Cannot finalize twice.
    assert!(
        session.finalize().is_err(),
        "should not finalize twice"
    );
}

// ---------------------------------------------------------------------------
// Test 7: MockChain trace output
// ---------------------------------------------------------------------------

#[test]
fn test_mockchain_trace_output() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("trace_output");

    let wallet_id = AccountId(0x1000);
    let faucet_id = AccountId(0x2000);
    let note_id = NoteId(1);

    let config = MockChainConfig {
        wallets: vec![WalletConfig {
            id: wallet_id,
            initial_assets: AssetVault::default(),
        }],
        faucets: vec![FaucetConfig {
            id: faucet_id,
            symbol: "TOKEN".to_string(),
            max_supply: 500_000,
            initial_supply: 100,
        }],
        p2id_notes: vec![P2IdNoteConfig {
            id: note_id,
            sender: faucet_id,
            receiver: wallet_id,
            assets: AssetVault {
                fungible: vec![FungibleAsset {
                    faucet_id,
                    amount: 50,
                }],
            },
            note_type: NoteType::Public,
            aux: 0,
        }],
        ..Default::default()
    };

    let mut session = ContractTraceSession::new(config, out_dir.clone());
    session.build_chain().unwrap();

    // Execute with a transaction script.
    let tx_config = TransactionConfig {
        account_id: wallet_id,
        input_notes: vec![note_id],
        tx_script: Some("begin\n    push.1\n    drop\nend".to_string()),
        debug_mode: true,
    };
    session.execute_transaction(&tx_config).unwrap();

    let summary_path = session.finalize().unwrap();

    // Read and verify the summary.
    let summary_content =
        std::fs::read_to_string(&summary_path).expect("failed to read summary");
    let summary: serde_json::Value =
        serde_json::from_str(&summary_content).expect("summary should be valid JSON");

    // Verify summary fields.
    assert!(
        summary["context_count"].as_u64().unwrap() >= 2,
        "should have multiple contexts"
    );
    assert!(
        summary["event_count"].as_u64().unwrap() > 0,
        "should have events"
    );
    assert!(
        summary["context_switches"].as_u64().unwrap() > 0,
        "should have context switches"
    );
    assert_eq!(
        summary["accounts"].as_u64().unwrap(),
        2,
        "should have 2 accounts"
    );
    assert_eq!(
        summary["notes"].as_u64().unwrap(),
        1,
        "should have 1 note"
    );

    // Verify kernel calls are listed.
    let kernel_calls = summary["kernel_calls"].as_array().unwrap();
    assert!(!kernel_calls.is_empty(), "should list kernel calls");

    // Should include prologue and epilogue.
    let call_names: Vec<&str> = kernel_calls.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        call_names.contains(&"#sys::prologue"),
        "should have prologue"
    );
    assert!(
        call_names.contains(&"#sys::epilogue"),
        "should have epilogue"
    );

    // Should include note-related kernel calls.
    assert!(
        call_names
            .iter()
            .any(|n| n.contains("miden::note::") || n.contains("miden::kernel::")),
        "should have note/kernel system calls"
    );

    // Verify the trace events contain the expected phases.
    let events = session.trace_events();

    // Should have steps in note script and account code.
    let steps: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::Step { .. }))
        .collect();
    assert!(!steps.is_empty(), "should have step events");

    // Should have variables captured.
    let vars: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::Variable { .. }))
        .collect();
    assert!(!vars.is_empty(), "should have variable events");

    // With tx_script set, should have a TxScript context switch.
    let tx_script_switches: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::ContextSwitch { kind: ContextKind::TxScript, .. }))
        .collect();
    assert!(
        !tx_script_switches.is_empty(),
        "should have tx_script context switch when tx_script is provided"
    );

    // Verify the chain recorded the transaction.
    let chain = session.chain().unwrap();
    let consumed_note = chain.get_note(&note_id).unwrap();
    assert!(consumed_note.consumed, "note should be consumed after transaction");
}

// ---------------------------------------------------------------------------
// Test: Transaction with missing account fails
// ---------------------------------------------------------------------------

#[test]
fn test_transaction_missing_account_fails() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let config = MockChainConfig {
        wallets: vec![WalletConfig {
            id: AccountId(0x1000),
            initial_assets: AssetVault::default(),
        }],
        ..Default::default()
    };

    let mut session = ContractTraceSession::new(config, tmp_dir.path().to_path_buf());
    session.build_chain().unwrap();

    let tx = TransactionConfig {
        account_id: AccountId(0x9999), // Does not exist
        input_notes: vec![],
        tx_script: None,
        debug_mode: true,
    };

    let result = session.execute_transaction(&tx);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

// ---------------------------------------------------------------------------
// Test: Transaction with missing note fails
// ---------------------------------------------------------------------------

#[test]
fn test_transaction_missing_note_fails() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let config = MockChainConfig {
        wallets: vec![WalletConfig {
            id: AccountId(0x1000),
            initial_assets: AssetVault::default(),
        }],
        ..Default::default()
    };

    let mut session = ContractTraceSession::new(config, tmp_dir.path().to_path_buf());
    session.build_chain().unwrap();

    let tx = TransactionConfig {
        account_id: AccountId(0x1000),
        input_notes: vec![NoteId(999)], // Does not exist
        tx_script: None,
        debug_mode: true,
    };

    let result = session.execute_transaction(&tx);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

// ---------------------------------------------------------------------------
// Test: MockChainBuilder with multiple notes and swap notes
// ---------------------------------------------------------------------------

#[test]
fn test_mockchain_builder_multiple_notes() {
    let wallet_a = AccountId(0x1000);
    let wallet_b = AccountId(0x2000);
    let faucet = AccountId(0x3000);

    let chain = MockChainBuilder::new()
        .add_existing_wallet(WalletConfig {
            id: wallet_a,
            initial_assets: AssetVault::default(),
        })
        .add_existing_wallet(WalletConfig {
            id: wallet_b,
            initial_assets: AssetVault::default(),
        })
        .add_existing_basic_faucet(FaucetConfig {
            id: faucet,
            symbol: "SWAP".to_string(),
            max_supply: 1_000_000,
            initial_supply: 0,
        })
        .add_p2id_note(P2IdNoteConfig {
            id: NoteId(1),
            sender: faucet,
            receiver: wallet_a,
            assets: AssetVault {
                fungible: vec![FungibleAsset {
                    faucet_id: faucet,
                    amount: 100,
                }],
            },
            note_type: NoteType::Public,
            aux: 0,
        })
        .add_output_note(OutputNote {
            id: NoteId(2),
            sender: wallet_a,
            assets: AssetVault {
                fungible: vec![FungibleAsset {
                    faucet_id: faucet,
                    amount: 50,
                }],
            },
            note_type: NoteType::Private,
        })
        .build();

    assert_eq!(chain.account_count(), 3);
    assert_eq!(chain.note_count(), 2);

    let note_1 = chain.get_note(&NoteId(1)).unwrap();
    assert_eq!(note_1.note_type, NoteType::Public);

    let note_2 = chain.get_note(&NoteId(2)).unwrap();
    assert_eq!(note_2.note_type, NoteType::Private);
    assert_eq!(note_2.sender, wallet_a);
}

// ---------------------------------------------------------------------------
// Test: Block production
// ---------------------------------------------------------------------------

#[test]
fn test_mockchain_block_production() {
    let wallet_id = AccountId(0x1000);
    let faucet_id = AccountId(0x2000);

    let mut chain = MockChainBuilder::new()
        .add_existing_wallet(WalletConfig {
            id: wallet_id,
            initial_assets: AssetVault::default(),
        })
        .add_existing_basic_faucet(FaucetConfig {
            id: faucet_id,
            symbol: "BLK".to_string(),
            max_supply: 1_000_000,
            initial_supply: 0,
        })
        .build();

    assert_eq!(chain.block_number, 0);
    assert!(chain.blocks.is_empty());

    // Record a transaction and produce a block.
    chain.record_transaction(TransactionRecord {
        account_id: wallet_id,
        consumed_notes: vec![],
        success: true,
    });
    chain.produce_block();

    assert_eq!(chain.block_number, 1);
    assert_eq!(chain.blocks.len(), 1);
    assert_eq!(chain.blocks[0].transactions.len(), 1);
    assert!(chain.blocks[0].transactions[0].success);
}

// ---------------------------------------------------------------------------
// Test: Execution context source path tracking
// ---------------------------------------------------------------------------

#[test]
fn test_execution_context_source_path() {
    let mut tracker = ExecutionContextTracker::new();

    tracker.switch_context(ContextId(0), ContextKind::Kernel, 0);
    tracker.set_source_path(ContextId(0), std::path::PathBuf::from("kernel.masm"));

    tracker.switch_context(
        ContextId(1),
        ContextKind::AccountCode {
            account_id: AccountId(0x1000),
        },
        100,
    );
    tracker.set_source_path(ContextId(1), std::path::PathBuf::from("account.masm"));

    let kernel_ctx = tracker.get_context(&ContextId(0)).unwrap();
    assert_eq!(
        kernel_ctx.source_path.as_deref(),
        Some(std::path::Path::new("kernel.masm"))
    );

    let account_ctx = tracker.get_context(&ContextId(1)).unwrap();
    assert_eq!(
        account_ctx.source_path.as_deref(),
        Some(std::path::Path::new("account.masm"))
    );
}

// ---------------------------------------------------------------------------
// Test: Swap notes are included in the built chain
// ---------------------------------------------------------------------------

#[test]
fn test_mockchain_builder_swap_notes() {
    let wallet_a = AccountId(0x1000);
    let wallet_b = AccountId(0x2000);
    let faucet = AccountId(0x3000);

    let chain = MockChainBuilder::new()
        .add_existing_wallet(WalletConfig {
            id: wallet_a,
            initial_assets: AssetVault::default(),
        })
        .add_existing_wallet(WalletConfig {
            id: wallet_b,
            initial_assets: AssetVault::default(),
        })
        .add_existing_basic_faucet(FaucetConfig {
            id: faucet,
            symbol: "SWAP".to_string(),
            max_supply: 1_000_000,
            initial_supply: 0,
        })
        .add_swap_note(SwapNoteConfig {
            id: NoteId(10),
            sender: wallet_a,
            offered: AssetVault {
                fungible: vec![FungibleAsset {
                    faucet_id: faucet,
                    amount: 100,
                }],
            },
            requested: AssetVault {
                fungible: vec![FungibleAsset {
                    faucet_id: faucet,
                    amount: 200,
                }],
            },
            note_type: NoteType::Public,
            aux: 0,
        })
        .build();

    assert_eq!(chain.note_count(), 1, "swap note should be in the chain");
    let note = chain.get_note(&NoteId(10)).expect("swap note should exist");
    assert_eq!(note.sender, wallet_a);
    assert_eq!(note.assets.fungible.len(), 1);
    assert_eq!(note.assets.fungible[0].amount, 100, "should use offered assets");
}

// ---------------------------------------------------------------------------
// Test: Multiple transactions accumulate in the same pending block
// ---------------------------------------------------------------------------

#[test]
fn test_mockchain_multiple_transactions_same_block() {
    let wallet_id = AccountId(0x1000);
    let faucet_id = AccountId(0x2000);

    let mut chain = MockChainBuilder::new()
        .add_existing_wallet(WalletConfig {
            id: wallet_id,
            initial_assets: AssetVault::default(),
        })
        .add_existing_basic_faucet(FaucetConfig {
            id: faucet_id,
            symbol: "BLK".to_string(),
            max_supply: 1_000_000,
            initial_supply: 0,
        })
        .build();

    // Record two transactions before producing a block.
    chain.record_transaction(TransactionRecord {
        account_id: wallet_id,
        consumed_notes: vec![],
        success: true,
    });
    chain.record_transaction(TransactionRecord {
        account_id: faucet_id,
        consumed_notes: vec![],
        success: true,
    });

    assert_eq!(chain.blocks.len(), 1, "both txns should be in one pending block");
    assert_eq!(chain.blocks[0].transactions.len(), 2, "block should have 2 transactions");

    chain.produce_block();
    assert_eq!(chain.block_number, 1);
}
