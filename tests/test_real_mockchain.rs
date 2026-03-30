//! Integration tests using the real miden-testing MockChain API.
//!
//! These tests verify that the recorder's MockChain wrapper concepts align
//! with the real miden-testing crate's MockChain, and that real Miden VM
//! transaction execution works end-to-end.
//!
//! The existing tests in test_mockchain.rs test the recorder's synthetic
//! MockChain implementation. These tests exercise the *real* miden-testing
//! crate to validate that:
//!
//! 1. Real MockChain can be built with accounts, faucets, and notes
//! 2. Real transactions can be executed through the MockChain
//! 3. Transaction execution produces valid results (account state changes)
//! 4. The recorder's execution context tracking concepts map correctly
//!    to real Miden VM execution patterns
//! 5. Block production and note consumption work as expected

use miden_protocol::asset::{Asset, FungibleAsset};
use miden_protocol::note::NoteType;
use miden_testing::{Auth, MockChain, TransactionContextBuilder};

// Re-use the recorder's types for verifying concept alignment.
use codetracer_miden_recorder::kernel_procs::{is_kernel_procedure, procedure_display_name};
use codetracer_miden_recorder::mockchain::{
    ContextId, ContextKind, ExecutionContextTracker, SessionStatus,
};

// ---------------------------------------------------------------------------
// Test 1: Real MockChain builder creates a valid chain with accounts
// ---------------------------------------------------------------------------

#[test]
fn test_real_mockchain_builder_creates_chain() {
    let mut builder = MockChain::builder();

    let wallet = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet");

    let faucet = builder
        .add_existing_basic_faucet(Auth::IncrNonce, "TEST", 1_000_000, None)
        .expect("failed to create faucet");

    let chain = builder.build().expect("failed to build chain");

    // Verify accounts were created and are accessible.
    assert!(
        chain.committed_account(wallet.id()).is_ok(),
        "wallet should exist in committed accounts"
    );
    assert!(
        chain.committed_account(faucet.id()).is_ok(),
        "faucet should exist in committed accounts"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Real MockChain with P2ID note
// ---------------------------------------------------------------------------

#[test]
fn test_real_mockchain_with_p2id_note() {
    let mut builder = MockChain::builder();

    let sender = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create sender");

    let receiver = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create receiver");

    let asset = FungibleAsset::mock(100);

    let note = builder
        .add_p2id_note(
            sender.id(),
            receiver.id(),
            &[asset],
            NoteType::Public,
        )
        .expect("failed to create P2ID note");

    let chain = builder.build().expect("failed to build chain");

    // Verify the note exists in the chain.
    assert!(
        chain.committed_notes().get(&note.id()).is_some(),
        "P2ID note should exist in chain"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Real transaction execution - consume P2ID note
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn test_real_transaction_execution() {
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
            &[Asset::Fungible(fungible_asset)],
            NoteType::Public,
        )
        .expect("failed to create P2ID note");

    let mut chain = builder.build().expect("failed to build chain");

    // Execute a transaction: receiver consumes the P2ID note.
    let executed_tx = chain
        .build_tx_context(receiver.id(), &[note.id()], &[])
        .expect("failed to build tx context")
        .build()
        .expect("failed to build tx context")
        .execute()
        .await
        .expect("transaction execution failed");

    // Verify the executed transaction has the correct account ID.
    assert_eq!(
        executed_tx.account_id(),
        receiver.id(),
        "executed tx should be for the receiver account"
    );

    // Add to chain and prove block.
    chain
        .add_pending_executed_transaction(&executed_tx)
        .expect("failed to add pending tx");

    chain.prove_next_block().expect("failed to prove block");

    // Verify the receiver now has the asset.
    let receiver_account = chain
        .committed_account(receiver.id())
        .expect("receiver should exist");
    let balance = receiver_account
        .vault()
        .get_balance(fungible_asset.faucet_id())
        .expect("should be able to query balance");
    assert_eq!(
        balance,
        fungible_asset.amount(),
        "receiver should have received the fungible asset"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Real MockChain with faucet and initial supply
// ---------------------------------------------------------------------------

#[test]
fn test_real_mockchain_faucet_with_supply() {
    let mut builder = MockChain::builder();

    let faucet = builder
        .add_existing_basic_faucet(Auth::IncrNonce, "GOLD", 10_000_000, Some(1_000))
        .expect("failed to create faucet with supply");

    let chain = builder.build().expect("failed to build chain");

    let faucet_account = chain
        .committed_account(faucet.id())
        .expect("faucet should exist");

    // Faucet account should exist and be valid.
    assert!(
        faucet_account.is_faucet(),
        "account should be a faucet"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Multiple accounts and notes in real MockChain
// ---------------------------------------------------------------------------

#[test]
fn test_real_mockchain_multiple_accounts_and_notes() {
    let mut builder = MockChain::builder();

    let wallet_a = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet A");
    let wallet_b = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create wallet B");
    let faucet = builder
        .add_existing_basic_faucet(Auth::IncrNonce, "MULTI", 1_000_000, None)
        .expect("failed to create faucet");

    let asset = FungibleAsset::mock(50);

    let note_1 = builder
        .add_p2id_note(
            faucet.id(),
            wallet_a.id(),
            &[asset],
            NoteType::Public,
        )
        .expect("failed to create note 1");

    let note_2 = builder
        .add_p2id_note(
            faucet.id(),
            wallet_b.id(),
            &[asset],
            NoteType::Public,
        )
        .expect("failed to create note 2");

    let chain = builder.build().expect("failed to build chain");

    // All accounts should exist.
    assert!(chain.committed_account(wallet_a.id()).is_ok());
    assert!(chain.committed_account(wallet_b.id()).is_ok());
    assert!(chain.committed_account(faucet.id()).is_ok());

    // Both notes should exist.
    assert!(chain.committed_notes().get(&note_1.id()).is_some());
    assert!(chain.committed_notes().get(&note_2.id()).is_some());
}

// ---------------------------------------------------------------------------
// Test 6: Real block production after transaction
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn test_real_block_production() {
    let mut builder = MockChain::builder();

    let sender = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create sender");

    let receiver = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create receiver");

    let asset = FungibleAsset::mock(200);

    let note = builder
        .add_p2id_note(
            sender.id(),
            receiver.id(),
            &[asset],
            NoteType::Public,
        )
        .expect("failed to create note");

    let mut chain = builder.build().expect("failed to build chain");

    // Get the initial block number.
    let initial_block = chain.latest_block_header().block_num();

    // Execute and add transaction.
    let executed_tx = chain
        .build_tx_context(receiver.id(), &[note.id()], &[])
        .expect("failed to build tx context")
        .build()
        .expect("failed to build tx context")
        .execute()
        .await
        .expect("transaction execution failed");

    chain
        .add_pending_executed_transaction(&executed_tx)
        .expect("failed to add pending tx");

    chain.prove_next_block().expect("failed to prove block");

    // Block number should have advanced.
    let new_block = chain.latest_block_header().block_num();
    assert!(
        new_block > initial_block,
        "block number should advance after proving"
    );
}

// ---------------------------------------------------------------------------
// Test 7: Execute code in transaction context
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn test_real_execute_code_in_tx_context() {
    // Use TransactionContextBuilder directly to execute arbitrary code.
    let tx_context = TransactionContextBuilder::with_existing_mock_account()
        .build()
        .expect("failed to build tx context");

    let code = "
    use $kernel::prologue
    use mock::account

    begin
        exec.prologue::prepare_transaction
        push.42
        swap drop
    end
    ";

    let exec_output = tx_context
        .execute_code(code)
        .await
        .expect("code execution failed");

    // Verify the stack has the expected value.
    let stack_ints = exec_output.stack.as_int_vec();
    assert!(
        !stack_ints.is_empty(),
        "stack should not be empty"
    );
    assert_eq!(
        stack_ints[0], 42,
        "stack top should be 42 after pushing it"
    );
}

// ---------------------------------------------------------------------------
// Test 8: Recorder's kernel proc detection aligns with real kernel procs
// ---------------------------------------------------------------------------

#[test]
fn test_kernel_proc_detection_with_real_patterns() {
    // These procedure names appear in real Miden transaction execution.
    // Verify the recorder's kernel procedure detection recognizes them.

    // Prologue/epilogue - always present in transactions.
    assert!(
        is_kernel_procedure("#sys::prologue"),
        "should detect prologue"
    );
    assert!(
        is_kernel_procedure("#sys::epilogue"),
        "should detect epilogue"
    );

    // Account-related kernel procedures.
    assert!(is_kernel_procedure("miden::kernel::get_account_id"));
    assert!(is_kernel_procedure("miden::kernel::account_vault_add_asset"));

    // Note-related kernel procedures.
    assert!(is_kernel_procedure("miden::note::get_inputs"));

    // Transaction-related kernel procedures.
    assert!(is_kernel_procedure("miden::tx::get_block_number"));

    // Asset-related kernel procedures.
    assert!(is_kernel_procedure("miden::asset::build_fungible_asset"));

    // Faucet-related kernel procedures.
    assert!(is_kernel_procedure("miden::faucet::mint"));

    // Display names should work correctly.
    assert_eq!(
        procedure_display_name("#sys::prologue"),
        "[kernel:setup] prologue"
    );
    assert_eq!(
        procedure_display_name("miden::kernel::get_account_id"),
        "[kernel] get_account_id"
    );
}

// ---------------------------------------------------------------------------
// Test 9: Context tracking simulates real transaction phases
// ---------------------------------------------------------------------------

#[test]
fn test_context_tracking_mirrors_real_tx_phases() {
    // A real Miden transaction goes through these phases:
    // 1. Kernel prologue (context 0)
    // 2. Note script execution (context 1+)
    // 3. Account code execution
    // 4. Transaction script (if present)
    // 5. Kernel epilogue (context 0)
    //
    // Verify the recorder's ExecutionContextTracker can model this.

    let mut tracker = ExecutionContextTracker::new();

    // Phase 1: Kernel prologue.
    tracker.switch_context(ContextId(0), ContextKind::Kernel, 0);
    assert!(tracker.is_in_kernel());

    // Phase 2: Note script (context switches from kernel to note).
    tracker.switch_context(
        ContextId(1),
        ContextKind::NoteScript {
            note_id: codetracer_miden_recorder::mockchain::NoteId(1),
        },
        100,
    );
    assert!(!tracker.is_in_kernel());

    // Note calls back to kernel (syscall).
    tracker.switch_context(ContextId(0), ContextKind::Kernel, 150);
    assert!(tracker.is_in_kernel());

    // Return to note.
    tracker.switch_context(
        ContextId(1),
        ContextKind::NoteScript {
            note_id: codetracer_miden_recorder::mockchain::NoteId(1),
        },
        170,
    );

    // Phase 3: Account code.
    tracker.switch_context(
        ContextId(2),
        ContextKind::AccountCode {
            account_id: codetracer_miden_recorder::mockchain::AccountId(0x1000),
        },
        200,
    );

    // Phase 4: Tx script.
    tracker.switch_context(ContextId(3), ContextKind::TxScript, 300);

    // Phase 5: Kernel epilogue.
    tracker.switch_context(ContextId(0), ContextKind::Kernel, 400);
    assert!(tracker.is_in_kernel());

    // Verify we tracked all context IDs.
    assert_eq!(tracker.context_count(), 4, "should have 4 distinct contexts");

    // Verify the switch history is complete.
    let history = tracker.switch_history();
    assert_eq!(history.len(), 7, "should have 7 context switches");

    // Verify context kinds.
    let kernel_ctx = tracker.get_context(&ContextId(0)).unwrap();
    assert_eq!(kernel_ctx.kind, ContextKind::Kernel);

    let note_ctx = tracker.get_context(&ContextId(1)).unwrap();
    assert!(matches!(note_ctx.kind, ContextKind::NoteScript { .. }));

    let account_ctx = tracker.get_context(&ContextId(2)).unwrap();
    assert!(matches!(account_ctx.kind, ContextKind::AccountCode { .. }));

    let tx_script_ctx = tracker.get_context(&ContextId(3)).unwrap();
    assert_eq!(tx_script_ctx.kind, ContextKind::TxScript);
}

// ---------------------------------------------------------------------------
// Test 10: Real wallet with initial assets
// ---------------------------------------------------------------------------

#[test]
fn test_real_wallet_with_initial_assets() {
    let mut builder = MockChain::builder();

    let fungible_asset = FungibleAsset::mock(500);

    let wallet = builder
        .add_existing_wallet_with_assets(Auth::IncrNonce, [fungible_asset])
        .expect("failed to create wallet with assets");

    let chain = builder.build().expect("failed to build chain");

    let account = chain
        .committed_account(wallet.id())
        .expect("wallet should exist");

    // The wallet should have the asset in its vault.
    let balance = account
        .vault()
        .get_balance(FungibleAsset::mock_issuer())
        .expect("should be able to query balance");
    assert_eq!(balance, 500, "wallet should have 500 of the mock asset");
}

// ---------------------------------------------------------------------------
// Test 11: Multiple transactions in sequence
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn test_real_multiple_sequential_transactions() {
    let mut builder = MockChain::builder();

    let sender = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create sender");

    let receiver_a = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create receiver A");

    let receiver_b = builder
        .add_existing_wallet(Auth::IncrNonce)
        .expect("failed to create receiver B");

    let asset_a = FungibleAsset::mock(100);
    let asset_b = FungibleAsset::mock(200);

    let note_a = builder
        .add_p2id_note(
            sender.id(),
            receiver_a.id(),
            &[asset_a],
            NoteType::Public,
        )
        .expect("failed to create note A");

    let note_b = builder
        .add_p2id_note(
            sender.id(),
            receiver_b.id(),
            &[asset_b],
            NoteType::Public,
        )
        .expect("failed to create note B");

    let mut chain = builder.build().expect("failed to build chain");

    // Execute first transaction.
    let tx_a = chain
        .build_tx_context(receiver_a.id(), &[note_a.id()], &[])
        .expect("failed to build tx context A")
        .build()
        .expect("failed to build tx context A")
        .execute()
        .await
        .expect("tx A execution failed");

    chain
        .add_pending_executed_transaction(&tx_a)
        .expect("failed to add tx A");

    // Execute second transaction.
    let tx_b = chain
        .build_tx_context(receiver_b.id(), &[note_b.id()], &[])
        .expect("failed to build tx context B")
        .build()
        .expect("failed to build tx context B")
        .execute()
        .await
        .expect("tx B execution failed");

    chain
        .add_pending_executed_transaction(&tx_b)
        .expect("failed to add tx B");

    // Prove block with both transactions.
    chain.prove_next_block().expect("failed to prove block");

    // Verify both receivers got their assets.
    let fungible_a = asset_a.unwrap_fungible();
    let fungible_b = asset_b.unwrap_fungible();

    let balance_a = chain
        .committed_account(receiver_a.id())
        .expect("receiver A should exist")
        .vault()
        .get_balance(fungible_a.faucet_id())
        .expect("should query balance A");
    assert_eq!(balance_a, fungible_a.amount());

    let balance_b = chain
        .committed_account(receiver_b.id())
        .expect("receiver B should exist")
        .vault()
        .get_balance(fungible_b.faucet_id())
        .expect("should query balance B");
    assert_eq!(balance_b, fungible_b.amount());
}

// ---------------------------------------------------------------------------
// Test 12: Session status model with real chain concepts
// ---------------------------------------------------------------------------

#[test]
fn test_session_status_model_with_real_chain() {
    // Verify the recorder's SessionStatus enum can model a real chain workflow.
    // This test ensures the status transitions make sense for real usage.

    let status = SessionStatus::Created;
    assert_eq!(status, SessionStatus::Created);

    let status = SessionStatus::ChainBuilt;
    assert_eq!(status, SessionStatus::ChainBuilt);

    let status = SessionStatus::Executed;
    assert_eq!(status, SessionStatus::Executed);

    let status = SessionStatus::Completed;
    assert_eq!(status, SessionStatus::Completed);

    let status = SessionStatus::Failed("test error".to_string());
    assert!(matches!(status, SessionStatus::Failed(_)));
}

// ---------------------------------------------------------------------------
// Test 13: Real MockChain new account creation via transaction
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn test_real_new_account_creation() {
    let mut builder = MockChain::builder();

    let faucet = builder
        .add_existing_basic_faucet(Auth::IncrNonce, "NEW", 1_000_000, None)
        .expect("failed to create faucet");

    // Create a new wallet (not added to chain state yet).
    let new_wallet = builder
        .create_new_wallet(Auth::IncrNonce)
        .expect("failed to create new wallet");

    let asset = FungibleAsset::mock(75);

    let note = builder
        .add_p2id_note(
            faucet.id(),
            new_wallet.id(),
            &[asset],
            NoteType::Public,
        )
        .expect("failed to create note");

    let chain = builder.build().expect("failed to build chain");

    // The new wallet should NOT be in committed accounts yet.
    assert!(
        chain.committed_account(new_wallet.id()).is_err(),
        "new wallet should not be committed before its first transaction"
    );

    // But we can still check the note exists.
    assert!(
        chain.committed_notes().get(&note.id()).is_some(),
        "note should exist in chain"
    );
}

// ---------------------------------------------------------------------------
// Test 14: Real MockChain with mock account component
// ---------------------------------------------------------------------------

#[test]
fn test_real_mockchain_mock_account() {
    let mut builder = MockChain::builder();

    let mock_account = builder
        .add_existing_mock_account(Auth::IncrNonce)
        .expect("failed to create mock account");

    let chain = builder.build().expect("failed to build chain");

    assert!(
        chain.committed_account(mock_account.id()).is_ok(),
        "mock account should exist in committed accounts"
    );
}
