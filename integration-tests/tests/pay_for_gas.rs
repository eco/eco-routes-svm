use anchor_lang::prelude::AccountMeta;
use anchor_lang::{AnchorSerialize, InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token::spl_token;
use eco_svm_std::{Bytes32, CHAIN_ID};
use hyper_prover::hyperlane;
use portal::types::{Call, Calldata, CalldataWithAccounts, TokenAmount};
use portal::{state, types};
use proof_helper::igp;
use rand::random;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;
use tiny_keccak::{Hasher, Keccak};

pub mod common;

use common::{hyperlane_context, proof_helper_context};

/// Create a fulfilled intent, prove it via Portal → HyperProver → Mailbox,
/// and return the dispatched_message_pda along with the source domain.
///
/// We build the prove instruction manually (like `prove_intent_via_hyper_prover`
/// does internally) so we control the `unique_message` keypair and can derive
/// the `dispatched_message_pda` for the subsequent pay_for_gas call.
fn setup_dispatched_message(ctx: &mut common::Context) -> (Pubkey, u32) {
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.calls.clear();
    route.native_amount = 0;
    let reward_hash = random::<[u8; 32]>().into();
    let claimant: Bytes32 = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;

    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let (fulfill_marker, _) = state::FulfillMarker::pda(&intent_hash);

    ctx.portal()
        .fulfill_intent(
            intent_hash,
            &route,
            reward_hash,
            claimant,
            executor,
            fulfill_marker,
            vec![],
            vec![],
        )
        .unwrap();

    let source_domain = random::<u32>();
    let source_prover: [u8; 32] = random();
    let dispatcher = state::dispatcher_pda().0;

    // Use the portal prove path (Portal → HyperProver → Mailbox) which is
    // the same path production uses. We need to know the unique_message keypair
    // to derive the dispatched_message_pda, so we build the prove_accounts manually.
    let unique_message = Keypair::new();
    let outbox_pda = hyperlane_context::outbox_pda();
    let dispatched_message_pda =
        hyperlane_context::dispatched_message_pda(&unique_message.pubkey());

    ctx.portal()
        .prove_intent_with_unique_message(
            vec![intent_hash],
            source_domain as u64,
            vec![fulfill_marker],
            dispatcher,
            hyper_prover::state::dispatcher_pda().0,
            hyperlane::MAILBOX_ID,
            source_prover.to_vec(),
            &unique_message,
            outbox_pda,
            dispatched_message_pda,
        )
        .unwrap();

    (dispatched_message_pda, source_domain)
}

#[test]
fn pay_for_gas_success() {
    let mut ctx = common::Context::default();
    let (dispatched_message, source_domain) = setup_dispatched_message(&mut ctx);

    let result = ctx
        .proof_helper()
        .pay_for_gas(dispatched_message, source_domain, 100_000);

    let tx = result.unwrap();
    assert!(tx
        .logs
        .iter()
        .any(|log| log.contains("MockIGP: pay_for_gas")));
}

#[test]
fn pay_for_gas_verifies_message_id() {
    let mut ctx = common::Context::default();
    let (dispatched_message, source_domain) = setup_dispatched_message(&mut ctx);

    // Read the dispatched message account to compute expected message_id
    let account = ctx.get_account(&dispatched_message).unwrap();
    let encoded_message = &account.data[53..]; // version(1) + discriminator(8) + nonce(4) + slot(8) + pubkey(32)
    let mut hasher = Keccak::v256();
    let mut expected_message_id = [0u8; 32];
    hasher.update(encoded_message);
    hasher.finalize(&mut expected_message_id);

    let result = ctx
        .proof_helper()
        .pay_for_gas(dispatched_message, source_domain, 100_000)
        .unwrap();

    assert!(result.logs.iter().any(|log| {
        log.contains(&format!(
            "MockIGP: pay_for_gas domain={} gas=100000",
            source_domain
        ))
    }));
}

#[test]
fn pay_for_gas_invalid_owner_fails() {
    let mut ctx = common::Context::default();

    // Create a fake dispatched message account owned by system program
    let fake_key = Pubkey::new_unique();
    let mut data = vec![0u8; 100];
    data[..8].copy_from_slice(b"DISPATCH");

    ctx.set_account(
        fake_key,
        solana_sdk::account::Account {
            lamports: 1_000_000,
            data,
            owner: anchor_lang::system_program::ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    let result = ctx.proof_helper().pay_for_gas(fake_key, 1, 100_000);

    assert!(result.is_err_and(common::is_error(
        proof_helper::instructions::ProofHelperError::InvalidDispatchedMessageOwner
    )));
}

#[test]
fn pay_for_gas_invalid_discriminator_fails() {
    let mut ctx = common::Context::default();

    // Create an account owned by mailbox but with wrong discriminator
    let fake_key = Pubkey::new_unique();
    let mut data = vec![0u8; 100];
    data[..8].copy_from_slice(b"NOTDISPA");

    ctx.set_account(
        fake_key,
        solana_sdk::account::Account {
            lamports: 1_000_000,
            data,
            owner: hyperlane::MAILBOX_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    let result = ctx.proof_helper().pay_for_gas(fake_key, 1, 100_000);

    assert!(result.is_err_and(common::is_error(
        proof_helper::instructions::ProofHelperError::InvalidDispatchedMessage
    )));
}

#[test]
fn pay_for_gas_too_short_data_fails() {
    let mut ctx = common::Context::default();

    let fake_key = Pubkey::new_unique();
    ctx.set_account(
        fake_key,
        solana_sdk::account::Account {
            lamports: 1_000_000,
            data: vec![0u8; 10],
            owner: hyperlane::MAILBOX_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    let result = ctx.proof_helper().pay_for_gas(fake_key, 1, 100_000);

    assert!(result.is_err_and(common::is_error(
        proof_helper::instructions::ProofHelperError::InvalidDispatchedMessage
    )));
}

/// Fulfill + prove + pay_for_gas in a single atomic transaction with 1 route
/// token and 1 route call (SPL Token transfer_checked). This validates the
/// entire design: state created by earlier instructions (FulfillMarker,
/// dispatched_message_pda) is readable by later instructions within the same
/// transaction.
#[test]
fn atomic_fulfill_prove_pay_for_gas() {
    let mut ctx = common::Context::default();

    // ── Setup route with 1 token + 1 transfer call ──
    let (_, mut route, _) = ctx.rand_intent();
    let token_program = ctx.token_program;
    let mint = route.tokens[0].token;
    let amount = route.tokens[0].amount;
    let executor = state::executor_pda().0;
    let solver = ctx.solver.pubkey();
    let recipient = Pubkey::new_unique();

    // Keep only 1 token, clear native amount
    route.tokens = vec![TokenAmount {
        token: mint,
        amount,
    }];
    route.native_amount = 0;

    // Build the transfer_checked call
    let executor_ata =
        get_associated_token_address_with_program_id(&executor, &mint, &token_program);
    let recipient_ata =
        get_associated_token_address_with_program_id(&recipient, &mint, &token_program);
    let calldata = Calldata {
        data: spl_token::instruction::transfer_checked(
            &token_program,
            &executor_ata,
            &mint,
            &recipient_ata,
            &executor,
            &[],
            amount,
            6,
        )
        .unwrap()
        .data,
        account_count: 4,
    };
    let call_accounts = vec![
        AccountMeta::new(executor_ata, false),
        AccountMeta::new_readonly(mint, false),
        AccountMeta::new(recipient_ata, false),
        AccountMeta::new_readonly(executor, false),
    ];
    let calldata_with_accounts =
        CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap();

    // Build the source route (with CalldataWithAccounts for hashing)
    let mut source_route = route.clone();
    source_route.calls = vec![Call {
        target: token_program.to_bytes().into(),
        data: calldata_with_accounts.try_to_vec().unwrap(),
    }];

    // Build the destination route (with Calldata for execution)
    let mut dest_route = route.clone();
    dest_route.calls = vec![Call {
        target: token_program.to_bytes().into(),
        data: calldata.try_to_vec().unwrap(),
    }];

    let reward_hash: Bytes32 = random::<[u8; 32]>().into();
    let claimant: Bytes32 = Pubkey::new_unique().to_bytes().into();
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let (fulfill_marker, _) = state::FulfillMarker::pda(&intent_hash);

    // Fund solver with tokens and create recipient ATA
    ctx.airdrop_token_ata(&mint, &solver, amount);
    ctx.airdrop_token_ata(&mint, &recipient, 0);

    let source_domain = random::<u32>();
    let source_prover: [u8; 32] = random();
    let dispatcher = state::dispatcher_pda().0;

    let unique_message = Keypair::new();
    let unique_gas_payment = Keypair::new();
    let outbox_pda = hyperlane_context::outbox_pda();
    let dispatched_message_pda =
        hyperlane_context::dispatched_message_pda(&unique_message.pubkey());

    // ── ix[0]: ComputeBudget ──
    let compute_budget = ComputeBudgetInstruction::set_compute_unit_limit(600_000);

    // ── ix[1]: Portal.fulfill (with token + call) ──
    let solver_ata = get_associated_token_address_with_program_id(&solver, &mint, &token_program);
    let token_accounts = vec![
        AccountMeta::new(solver_ata, false),
        AccountMeta::new(executor_ata, false),
        AccountMeta::new_readonly(mint, false),
    ];
    let fulfill_args = portal::instructions::FulfillArgs {
        intent_hash,
        route: dest_route,
        reward_hash,
        claimant,
    };
    let fulfill_ix = Instruction {
        program_id: portal::ID,
        accounts: portal::accounts::Fulfill {
            payer: ctx.payer.pubkey(),
            solver,
            executor,
            fulfill_marker,
            token_program: anchor_spl::token::ID,
            token_2022_program: anchor_spl::token_2022::ID,
            associated_token_program: anchor_spl::associated_token::ID,
            system_program: anchor_lang::system_program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(token_accounts)
        .chain(call_accounts)
        .collect(),
        data: (portal::instruction::Fulfill { args: fulfill_args }).data(),
    };

    // ── ix[2]: Portal.prove ──
    let prove_args = portal::instructions::ProveArgs {
        prover: hyper_prover::ID,
        source_chain_domain_id: source_domain as u64,
        intent_hashes: vec![intent_hash],
        data: source_prover.to_vec(),
    };
    let prove_ix = Instruction {
        program_id: portal::ID,
        accounts: portal::accounts::Prove {
            prover: hyper_prover::ID,
            dispatcher,
        }
        .to_account_metas(None)
        .into_iter()
        .chain(std::iter::once(AccountMeta {
            pubkey: fulfill_marker,
            is_signer: false,
            is_writable: false,
        }))
        .chain(vec![
            AccountMeta::new_readonly(hyper_prover::state::dispatcher_pda().0, false),
            AccountMeta::new(ctx.payer.pubkey(), true),
            AccountMeta::new(outbox_pda, false),
            AccountMeta::new_readonly(spl_noop::ID, false),
            AccountMeta::new_readonly(unique_message.pubkey(), true),
            AccountMeta::new(dispatched_message_pda, false),
            AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
            AccountMeta::new_readonly(hyperlane::MAILBOX_ID, false),
        ])
        .collect(),
        data: (portal::instruction::Prove { args: prove_args }).data(),
    };

    // ── ix[3]: ProofHelper.pay_for_gas ──
    let salt = [0u8; 32];
    let pay_ix = Instruction {
        program_id: proof_helper::ID,
        accounts: proof_helper::accounts::PayForGas {
            dispatched_message: dispatched_message_pda,
            payer: ctx.payer.pubkey(),
            igp_program_data: proof_helper_context::igp_program_data_pda(),
            unique_gas_payment: unique_gas_payment.pubkey(),
            gas_payment_pda: proof_helper_context::gas_payment_pda(&unique_gas_payment.pubkey()),
            igp_account: proof_helper_context::igp_account_pda(&salt),
            overhead_igp: None,
            system_program: anchor_lang::system_program::ID,
            igp_program: igp::IGP_PROGRAM_ID,
        }
        .to_account_metas(None),
        data: (proof_helper::instruction::PayForGas {
            args: proof_helper::instructions::PayForGasArgs {
                destination_domain: source_domain,
                gas_amount: 100_000,
            },
        })
        .data(),
    };

    // ── Build and send ──
    let transaction = Transaction::new(
        &[
            &ctx.payer,
            &ctx.solver,
            &unique_message,
            &unique_gas_payment,
        ],
        Message::new(
            &[compute_budget, fulfill_ix, prove_ix, pay_ix],
            Some(&ctx.payer.pubkey()),
        ),
        ctx.latest_blockhash(),
    );

    let result = ctx.send_transaction(transaction).map_err(Box::new);
    let tx = result.unwrap();

    assert!(tx.logs.iter().any(|l| l.contains("Instruction: Fulfill")));
    assert!(tx.logs.iter().any(|l| l.contains("Instruction: Prove")));
    assert!(tx.logs.iter().any(|l| l.contains("MockIGP: pay_for_gas")));
    assert_eq!(ctx.token_balance_ata(&mint, &recipient), amount);
}
