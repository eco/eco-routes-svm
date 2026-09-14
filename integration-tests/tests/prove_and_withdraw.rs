use anchor_lang::prelude::{borsh, AccountMeta};
use anchor_lang::Discriminator;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token::spl_token;
use eco_svm_std::prover::{IntentProven, Proof};
use eco_svm_std::{Bytes32, CHAIN_ID};
use flash_fulfiller::instructions::{FlashFulfillIntent, FlashFulfillerError};
use portal::instructions::PortalError;
use portal::state::{executor_pda, vault_pda, FulfillMarker, WithdrawnMarker};
use portal::types::{
    intent_hash, Call, Calldata, CalldataWithAccounts, Reward, Route, TokenAmount,
};
use solana_sdk::instruction::{Instruction, InstructionError};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::TransactionError;

pub mod common;

const RELAY_BIN: &[u8] = include_bytes!("../../target/deploy/cpi_relay.so");

fn setup_with_context(mut ctx: common::Context) -> (common::Context, Route, Reward) {
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    reward.tokens.truncate(1);
    route.calls.clear();
    route.native_amount = reward.native_amount / 2;
    route.tokens = reward
        .tokens
        .iter()
        .map(|token| TokenAmount {
            token: token.token,
            amount: token.amount / 2,
        })
        .collect();
    fund(&mut ctx, &route, &reward);
    (ctx, route, reward)
}

fn setup() -> (common::Context, Route, Reward) {
    setup_with_context(common::Context::default())
}

fn fund(ctx: &mut common::Context, route: &Route, reward: &Reward) {
    let vault = vault_pda(&hash(route, reward)).0;
    let funder = ctx.funder.pubkey();
    let solver = ctx.solver.pubkey();
    ctx.airdrop(&funder, reward.native_amount + common::sol_amount(1.0))
        .unwrap();
    for (mint, amount) in reward.token_amounts().unwrap() {
        ctx.airdrop_token_ata(&mint, &funder, amount);
        ctx.airdrop_token_ata(&mint, &solver, 0);
    }
    let transfers: Vec<_> = reward
        .token_amounts()
        .unwrap()
        .keys()
        .flat_map(|mint| ctx.token_transfer_metas(funder, vault, *mint))
        .collect();
    ctx.portal()
        .fund_intent(
            CHAIN_ID,
            reward.clone(),
            vault,
            route.hash(),
            false,
            transfers,
        )
        .unwrap();
}

fn hash(route: &Route, reward: &Reward) -> Bytes32 {
    intent_hash(CHAIN_ID, &route.hash(), &reward.hash())
}

fn pair(ctx: &common::Context, route: &Route, reward: &Reward) -> [Instruction; 2] {
    [
        ctx.build_prove_and_withdraw_instruction(route.hash(), reward),
        ctx.build_paired_fulfill_instruction(route, reward, vec![]),
    ]
}

fn send(ctx: &mut common::Context, instructions: &[Instruction]) -> common::TransactionResult {
    let tx = ctx.build_split_fulfill_transaction(instructions);
    ctx.send_transaction(tx)
}

fn assert_unsettled(ctx: &common::Context, route: &Route, reward: &Reward) {
    let hash = hash(route, reward);
    assert!(ctx
        .get_account(&Proof::pda(&hash, &reward.prover).0)
        .is_none());
    assert!(ctx.get_account(&WithdrawnMarker::pda(&hash).0).is_none());
    assert!(ctx.get_account(&FulfillMarker::pda(&hash).0).is_none());
    for (mint, amount) in reward.token_amounts().unwrap() {
        assert_eq!(ctx.token_balance_ata(&mint, &vault_pda(&hash).0), amount);
    }
}

fn assert_missing_pair(
    ctx: &mut common::Context,
    route: &Route,
    reward: &Reward,
    instructions: &[Instruction],
) {
    let failure = send(ctx, instructions).unwrap_err();
    assert!(
        common::is_error(FlashFulfillerError::MissingPairedFulfill)(failure.clone()),
        "{failure:?}"
    );
    // The guard must reject before the prove CPI mints a proof.
    assert!(!failure
        .meta
        .logs
        .iter()
        .any(|line| line == &format!("Program {} invoke [2]", local_prover::ID)));
    assert_unsettled(ctx, route, reward);
}

#[test]
fn no_paired_fulfill_rejects_standalone_reward_theft() {
    let (mut ctx, route, reward) = setup();
    let prove = ctx.build_prove_and_withdraw_instruction(route.hash(), &reward);
    assert_missing_pair(&mut ctx, &route, &reward, &[prove]);
}

#[test]
fn different_intent_hash_is_rejected() {
    let (mut ctx, route, reward) = setup();
    let [prove, mut fulfill] = pair(&ctx, &route, &reward);
    fulfill.data[8] ^= 1;
    assert_missing_pair(&mut ctx, &route, &reward, &[prove, fulfill]);
}

#[test]
fn different_solver_is_rejected() {
    let (mut ctx, route, reward) = setup();
    let [prove, mut fulfill] = pair(&ctx, &route, &reward);
    fulfill.accounts[1].pubkey = ctx.payer.pubkey();
    assert_missing_pair(&mut ctx, &route, &reward, &[prove, fulfill]);
}

#[test]
fn wrong_fulfill_marker_is_rejected() {
    let (mut ctx, route, reward) = setup();
    let [prove, mut fulfill] = pair(&ctx, &route, &reward);
    fulfill.accounts[3].pubkey = Pubkey::new_unique();
    assert_missing_pair(&mut ctx, &route, &reward, &[prove, fulfill]);
}

#[test]
fn wrong_discriminator_is_rejected() {
    let (mut ctx, route, reward) = setup();
    let [prove, mut fulfill] = pair(&ctx, &route, &reward);
    fulfill.data[0] ^= 1;
    assert_missing_pair(&mut ctx, &route, &reward, &[prove, fulfill]);
}

#[test]
fn right_discriminator_with_truncated_data_is_rejected_without_panic() {
    let (mut ctx, route, reward) = setup();
    let [prove, mut fulfill] = pair(&ctx, &route, &reward);
    fulfill.data.truncate(39);
    assert_eq!(
        &fulfill.data[..8],
        <portal::instruction::Fulfill as Discriminator>::DISCRIMINATOR
    );
    assert_missing_pair(&mut ctx, &route, &reward, &[prove, fulfill]);
}

#[test]
fn truncated_fulfill_accounts_are_rejected_without_panic() {
    let (mut ctx, route, reward) = setup();
    let [prove, mut fulfill] = pair(&ctx, &route, &reward);
    fulfill.accounts.truncate(3);
    assert_missing_pair(&mut ctx, &route, &reward, &[prove, fulfill]);
}

fn relay(ctx: &mut common::Context, instruction: Instruction) -> Instruction {
    let program_id = Pubkey::new_unique();
    ctx.add_program(program_id, RELAY_BIN).unwrap();
    Instruction {
        program_id,
        accounts: std::iter::once(AccountMeta::new_readonly(instruction.program_id, false))
            .chain(instruction.accounts)
            .collect(),
        data: instruction.data,
    }
}

fn prefund_solver(ctx: &mut common::Context, route: &Route) {
    let solver = ctx.solver.pubkey();
    ctx.airdrop(&solver, common::sol_amount(2.0)).unwrap();
    for token in &route.tokens {
        ctx.airdrop_token_ata(&token.token, &solver, token.amount);
    }
}

#[test]
fn fulfill_only_as_cpi_is_rejected_even_after_it_succeeds() {
    let (mut ctx, route, reward) = setup();
    prefund_solver(&mut ctx, &route);
    let [prove, fulfill] = pair(&ctx, &route, &reward);
    let wrapped = relay(&mut ctx, fulfill);
    let failure = send(&mut ctx, &[wrapped, prove]).unwrap_err();
    assert_eq!(
        failure.err,
        TransactionError::InstructionError(3, InstructionError::Custom(6010))
    );
    assert!(failure
        .meta
        .logs
        .contains(&format!("Program {} invoke [2]", portal::ID)));
    assert!(failure
        .meta
        .logs
        .contains(&format!("Program {} success", portal::ID)));
    assert!(common::is_error(FlashFulfillerError::MissingPairedFulfill)(
        failure
    ));
    assert_unsettled(&ctx, &route, &reward);
}

#[test]
fn forged_instructions_sysvar_is_rejected() {
    let (mut ctx, route, reward) = setup();
    let [mut prove, fulfill] = pair(&ctx, &route, &reward);
    let fake = Pubkey::new_unique();
    let real = ctx.get_account(&solana_instructions_sysvar::ID);
    // A caller-controlled account with arbitrary data cannot replace the runtime sysvar.
    ctx.set_account(
        fake,
        solana_sdk::account::Account {
            lamports: 1_000_000,
            data: real.map_or_else(Vec::new, |account| account.data),
            owner: anchor_lang::system_program::ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    prove.accounts[2].pubkey = fake;
    let failure = send(&mut ctx, &[prove, fulfill]).unwrap_err();
    assert!(common::is_error(
        anchor_lang::error::ErrorCode::ConstraintAddress
    )(failure));
    assert_unsettled(&ctx, &route, &reward);
}

fn assert_settled(ctx: &common::Context, route: &Route, reward: &Reward) {
    let hash = hash(route, reward);
    assert!(ctx.get_account(&WithdrawnMarker::pda(&hash).0).is_some());
    let marker = ctx
        .account::<FulfillMarker>(&FulfillMarker::pda(&hash).0)
        .unwrap();
    assert_eq!(marker.claimant, ctx.solver.pubkey());
    assert_eq!(marker.payer, ctx.payer.pubkey());
    assert!(ctx
        .get_account(&Proof::pda(&hash, &reward.prover).0)
        .is_none());
}

fn happy_path(ctx: common::Context) {
    let (mut ctx, route, reward) = setup_with_context(ctx);
    let instructions = pair(&ctx, &route, &reward);
    // The solver starts with no native or reward tokens; ix1 finances ix2.
    assert_eq!(ctx.balance(&ctx.solver.pubkey()), 0);
    let result = send(&mut ctx, &instructions).unwrap();
    assert!(common::contains_cpi_event(IntentProven::new(
        hash(&route, &reward),
        ctx.solver.pubkey(),
        CHAIN_ID,
    ))(result));
    for token in &reward.tokens {
        assert_eq!(
            ctx.token_balance_ata(&token.token, &ctx.solver.pubkey()),
            token.amount / 2
        );
        assert_eq!(
            ctx.token_balance_ata(&token.token, &executor_pda().0),
            token.amount / 2
        );
        assert_eq!(
            ctx.token_balance_ata(&token.token, &vault_pda(&hash(&route, &reward)).0),
            0
        );
    }
    assert_eq!(
        ctx.balance(&ctx.solver.pubkey()),
        reward.native_amount - route.native_amount
    );
    assert_settled(&ctx, &route, &reward);
}

#[test]
fn happy_path_reward_funds_fulfillment_and_proof_is_closed() {
    happy_path(common::Context::default());
}

#[test]
fn happy_path_token_2022() {
    happy_path(common::Context::new_with_token_2022());
}

#[test]
fn earlier_top_level_fulfill_is_accepted() {
    let (mut ctx, route, reward) = setup();
    prefund_solver(&mut ctx, &route);
    let [prove, fulfill] = pair(&ctx, &route, &reward);
    send(&mut ctx, &[fulfill, prove]).unwrap();
    assert_settled(&ctx, &route, &reward);
}

#[test]
fn later_failed_fulfill_rolls_back_proof_and_reward() {
    let (mut ctx, route, reward) = setup();
    let instructions = pair(&ctx, &route, &reward);
    ctx.warp_to_timestamp((route.deadline + 1).try_into().unwrap());
    let failure = send(&mut ctx, &instructions).unwrap_err();
    assert!(failure
        .meta
        .logs
        .contains(&format!("Program {} success", flash_fulfiller::ID)));
    assert!(common::is_error(PortalError::RouteExpired)(failure));
    assert_unsettled(&ctx, &route, &reward);
    assert_eq!(ctx.balance(&ctx.solver.pubkey()), 0);
    assert_eq!(
        ctx.token_balance_ata(&reward.tokens[0].token, &ctx.solver.pubkey()),
        0
    );
}

#[test]
fn matching_prefix_with_invalid_fulfill_tail_rolls_back_withdrawal() {
    let (mut ctx, route, reward) = setup();
    let [prove, mut fulfill] = pair(&ctx, &route, &reward);
    fulfill.data.truncate(40);
    let failure = send(&mut ctx, &[prove, fulfill]).unwrap_err();
    assert!(failure
        .meta
        .logs
        .contains(&format!("Program {} success", flash_fulfiller::ID)));
    assert!(common::is_error(
        anchor_lang::error::ErrorCode::InstructionDidNotDeserialize
    )(failure));
    assert_unsettled(&ctx, &route, &reward);
}

#[test]
fn replay_in_later_transaction_hits_portal_withdrawn_marker() {
    let (mut ctx, route, reward) = setup();
    let instructions = pair(&ctx, &route, &reward);
    send(&mut ctx, &instructions).unwrap();
    ctx.expire_blockhash();
    let failure = send(&mut ctx, &instructions).unwrap_err();
    assert_eq!(
        failure.err,
        TransactionError::InstructionError(
            2,
            InstructionError::Custom(u32::from(PortalError::IntentAlreadyWithdrawn)),
        )
    );
    assert!(common::is_error(PortalError::IntentAlreadyWithdrawn)(
        failure
    ));
    assert_settled(&ctx, &route, &reward);
    assert_eq!(
        ctx.token_balance_ata(&reward.tokens[0].token, &ctx.solver.pubkey()),
        reward.tokens[0].amount / 2
    );
}

#[test]
fn route_reaches_spl_token_at_depth_five_and_old_shape_hits_call_depth() {
    let mut ctx = common::Context::default().with_five_frame_limit();
    let (_, mut route, mut reward) = ctx.rand_intent();
    reward.prover = local_prover::ID;
    reward.tokens.truncate(1);
    reward.native_amount = 0;
    route.native_amount = 0;
    route.tokens = vec![TokenAmount {
        token: reward.tokens[0].token,
        amount: reward.tokens[0].amount / 2,
    }];
    let mint = route.tokens[0].token;
    let executor = executor_pda().0;
    let recipient = Pubkey::new_unique();
    ctx.airdrop_token_ata(&mint, &recipient, 0);
    let recipient_ata =
        get_associated_token_address_with_program_id(&recipient, &mint, &ctx.token_program);
    let executor_ata =
        get_associated_token_address_with_program_id(&executor, &mint, &ctx.token_program);
    let token_ix = spl_token::instruction::transfer_checked(
        &ctx.token_program,
        &executor_ata,
        &mint,
        &recipient_ata,
        &executor,
        &[],
        route.tokens[0].amount,
        6,
    )
    .unwrap();
    // portal[1] -> engine[2] -> venue[3] -> sub-program[4] -> SPL Token[5].
    let breaker = relay(&mut ctx, token_ix);
    let venue = relay(&mut ctx, breaker);
    let engine = relay(&mut ctx, venue);
    let mut call_accounts = engine.accounts;
    // Portal supplies the executor signature itself; don't require a PDA to
    // sign the top-level message. Its canonical route metadata records false.
    for meta in &mut call_accounts {
        if meta.pubkey == executor {
            meta.is_signer = false;
            meta.is_writable = true;
        }
    }
    call_accounts.push(AccountMeta::new_readonly(engine.program_id, false));
    let calldata = Calldata {
        data: engine.data,
        account_count: call_accounts.len() as u8,
    };
    route.calls = vec![Call {
        target: engine.program_id.to_bytes().into(),
        data: borsh::to_vec(&CalldataWithAccounts::new(calldata, call_accounts.clone()).unwrap())
            .unwrap(),
    }];
    fund(&mut ctx, &route, &reward);

    // Same committed route, same real program binaries, same feature set.
    let solver = ctx.solver.pubkey();
    let solver_ata =
        get_associated_token_address_with_program_id(&solver, &mint, &ctx.token_program);
    let failure = ctx
        .flash_fulfiller()
        .flash_fulfill(
            FlashFulfillIntent::Intent {
                route: route.clone(),
                reward: reward.clone(),
            },
            None,
            &route,
            &reward,
            solver,
            vec![AccountMeta::new(solver_ata, false)],
            call_accounts.clone(),
        )
        .unwrap_err();
    assert_eq!(
        failure.err,
        TransactionError::InstructionError(2, InstructionError::CallDepth)
    );
    assert_unsettled(&ctx, &route, &reward);

    let instructions = [
        ctx.build_prove_and_withdraw_instruction(route.hash(), &reward),
        ctx.build_paired_fulfill_instruction(&route, &reward, call_accounts),
    ];
    let result = send(&mut ctx, &instructions).unwrap();
    assert!(result
        .logs
        .contains(&format!("Program {} invoke [5]", spl_token::ID)));
    assert_eq!(ctx.token_balance(&recipient_ata), route.tokens[0].amount);
    assert_eq!(
        ctx.token_balance_ata(&mint, &solver),
        reward.tokens[0].amount - route.tokens[0].amount
    );
    assert_eq!(ctx.token_balance(&executor_ata), 0);
    assert_settled(&ctx, &route, &reward);
}
