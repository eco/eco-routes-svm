use anchor_lang::prelude::{borsh, AccountMeta};
use anchor_lang::{Discriminator, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token::spl_token;
use eco_svm_std::prover::{IntentProven, Proof};
use eco_svm_std::{Bytes32, CHAIN_ID};
use flash_fulfiller::instructions::{FlashFulfillIntent, FlashFulfillerError};
use local_prover::instructions::LocalProverError;
use portal::instructions::PortalError;
use portal::state::{executor_pda, vault_pda, FulfillMarker, WithdrawnMarker};
use portal::types::{
    intent_hash, Call, Calldata, CalldataWithAccounts, Reward, Route, TokenAmount,
};
use solana_packet::PACKET_DATA_SIZE;
use solana_sdk::instruction::{Instruction, InstructionError};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::TransactionError;

use crate::common::split_venue_context::{
    sha256_hex, spl_ata, split_venue_lookup_addresses, venue_programs_dir, VenueRoute,
    HELIUM_CIRCUIT_BREAKER_BIN, HELIUM_CIRCUIT_BREAKER_SHA256, HELIUM_TREASURY_MANAGEMENT_BIN,
    HELIUM_TREASURY_MANAGEMENT_SHA256, MAX_TX_ACCOUNT_LOCKS, SPLIT_VENUE_COMPUTE_LIMIT,
    VENUE_SNAPSHOT_UNIX_TIMESTAMP,
};

pub mod common;

const RELAY_BIN: &[u8] = include_bytes!("../../target/deploy/cpi_relay.so");

fn unfunded_with_context(mut ctx: common::Context) -> (common::Context, Route, Reward) {
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
    (ctx, route, reward)
}

fn setup_with_context(ctx: common::Context) -> (common::Context, Route, Reward) {
    let (mut ctx, route, reward) = unfunded_with_context(ctx);
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

/// Withdraw leaves the proof open; `close_proof` is portal's separate cleanup.
fn assert_settled(ctx: &common::Context, route: &Route, reward: &Reward) {
    let hash = hash(route, reward);
    assert!(ctx.get_account(&WithdrawnMarker::pda(&hash).0).is_some());
    let marker = ctx
        .account::<FulfillMarker>(&FulfillMarker::pda(&hash).0)
        .unwrap();
    assert_eq!(marker.claimant, ctx.solver.pubkey());
    assert!(ctx
        .get_account(&Proof::pda(&hash, &reward.prover).0)
        .is_some());
}

fn close_proof(ctx: &mut common::Context, route: &Route, reward: &Reward) {
    let proof = Proof::pda(&hash(route, reward), &reward.prover).0;
    let payer = ctx.payer.pubkey();
    ctx.portal()
        .close_proof(
            CHAIN_ID,
            route.hash(),
            reward.clone(),
            vec![
                AccountMeta::new(proof, false),
                AccountMeta::new(payer, true),
            ],
        )
        .unwrap();
    assert!(ctx.get_account(&proof).is_none());
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
    close_proof(&mut ctx, &route, &reward);
}

#[test]
fn happy_path_reward_funds_fulfillment_and_proof_is_closed() {
    happy_path(common::Context::default());
}

#[test]
fn happy_path_token_2022() {
    happy_path(common::Context::new_with_token_2022());
}

/// README step 4: portal's generated `Fulfill` metas leave `executor`
/// read-only, and a native route's funding transfer writes it.
#[test]
fn native_route_fails_without_writable_executor_meta() {
    let (mut ctx, route, reward) = setup();
    assert!(route.native_amount > 0);
    let generated = portal::accounts::Fulfill {
        payer: ctx.payer.pubkey(),
        solver: ctx.solver.pubkey(),
        executor: executor_pda().0,
        fulfill_marker: FulfillMarker::pda(&hash(&route, &reward)).0,
        token_program: anchor_spl::token::ID,
        token_2022_program: anchor_spl::token_2022::ID,
        associated_token_program: anchor_spl::associated_token::ID,
        system_program: anchor_lang::system_program::ID,
    }
    .to_account_metas(None);
    assert!(!generated[2].is_writable);
    let [prove, mut fulfill] = pair(&ctx, &route, &reward);
    assert!(fulfill.accounts[2].is_writable);
    fulfill.accounts[2].is_writable = generated[2].is_writable;

    let failure = send(&mut ctx, &[prove, fulfill]).unwrap_err();

    assert_eq!(
        failure.err,
        TransactionError::InstructionError(3, InstructionError::PrivilegeEscalation)
    );
    assert_unsettled(&ctx, &route, &reward);
}

/// Stocks the vault directly with `native` lamports and `tokens` of the reward
/// mint, and gives the solver its route inputs.
fn stock_vault(
    ctx: common::Context,
    native: impl FnOnce(&Reward) -> u64,
    tokens: impl FnOnce(&Reward) -> u64,
) -> (common::Context, Route, Reward) {
    let (mut ctx, route, reward) = unfunded_with_context(ctx);
    let vault = vault_pda(&hash(&route, &reward)).0;
    let native = native(&reward);
    if native > 0 {
        ctx.airdrop(&vault, native).unwrap();
    }
    let tokens = tokens(&reward);
    ctx.airdrop_token_ata(&reward.tokens[0].token, &vault, tokens);
    prefund_solver(&mut ctx, &route);
    (ctx, route, reward)
}

/// The pair fails in `prove_and_withdraw` and leaves the solver untouched.
fn assert_reward_not_paid_in_full(
    ctx: &mut common::Context,
    route: &Route,
    reward: &Reward,
    before_prove: bool,
) {
    let solver = ctx.solver.pubkey();
    let lamports = ctx.balance(&solver);
    let tokens: Vec<_> = route
        .tokens
        .iter()
        .map(|token| ctx.token_balance_ata(&token.token, &solver))
        .collect();
    let instructions = pair(ctx, route, reward);

    let failure = send(ctx, &instructions).unwrap_err();

    assert_eq!(
        failure.err,
        TransactionError::InstructionError(
            2,
            InstructionError::Custom(u32::from(FlashFulfillerError::RewardNotPaidInFull)),
        )
    );
    assert_eq!(
        before_prove,
        !failure
            .meta
            .logs
            .contains(&format!("Program {} invoke [2]", local_prover::ID))
    );
    let hash = hash(route, reward);
    assert!(ctx
        .get_account(&Proof::pda(&hash, &reward.prover).0)
        .is_none());
    assert!(ctx.get_account(&WithdrawnMarker::pda(&hash).0).is_none());
    assert!(ctx.get_account(&FulfillMarker::pda(&hash).0).is_none());
    assert_eq!(ctx.balance(&solver), lamports);
    for (token, before) in route.tokens.iter().zip(tokens) {
        assert_eq!(ctx.token_balance_ata(&token.token, &solver), before);
    }
}

#[test]
fn rejects_vault_without_native_reward() {
    let (mut ctx, route, reward) =
        stock_vault(common::Context::default(), |_| 0, |r| r.tokens[0].amount);
    assert_reward_not_paid_in_full(&mut ctx, &route, &reward, true);
}

#[test]
fn rejects_vault_without_token_reward() {
    let (mut ctx, route, reward) =
        stock_vault(common::Context::default(), |r| r.native_amount, |_| 0);
    assert_reward_not_paid_in_full(&mut ctx, &route, &reward, true);
}

#[test]
fn rejects_vault_short_of_native_reward() {
    let (mut ctx, route, reward) = stock_vault(
        common::Context::default(),
        |r| r.native_amount - 1,
        |r| r.tokens[0].amount,
    );
    assert_reward_not_paid_in_full(&mut ctx, &route, &reward, true);
}

#[test]
fn rejects_vault_short_of_token_reward() {
    let (mut ctx, route, reward) = stock_vault(
        common::Context::default(),
        |r| r.native_amount,
        |r| r.tokens[0].amount - 1,
    );
    assert_reward_not_paid_in_full(&mut ctx, &route, &reward, true);
}

#[test]
fn rejects_vault_short_of_token_2022_reward() {
    let (mut ctx, route, reward) = stock_vault(
        common::Context::new_with_token_2022(),
        |r| r.native_amount,
        |r| r.tokens[0].amount - 1,
    );
    assert_reward_not_paid_in_full(&mut ctx, &route, &reward, true);
}

#[test]
fn rejects_vault_refunded_after_the_reward_deadline() {
    let (mut ctx, mut route, mut reward) = unfunded_with_context(common::Context::default());
    reward.tokens.clear();
    route.tokens.clear();
    reward.deadline = ctx.now() + 600;
    fund(&mut ctx, &route, &reward);
    prefund_solver(&mut ctx, &route);
    let hash = hash(&route, &reward);
    ctx.warp_to_timestamp(reward.deadline as i64);
    ctx.portal()
        .refund_intent(
            CHAIN_ID,
            reward.clone(),
            vault_pda(&hash).0,
            route.hash(),
            Proof::pda(&hash, &reward.prover).0,
            WithdrawnMarker::pda(&hash).0,
            reward.creator,
            Vec::<AccountMeta>::new(),
        )
        .unwrap();
    assert_reward_not_paid_in_full(&mut ctx, &route, &reward, true);
}

#[test]
fn rejects_vault_short_of_one_of_two_reward_mints() {
    let (mut ctx, route, mut reward) = unfunded_with_context(common::Context::default());
    let second = Pubkey::new_unique();
    ctx.set_mint_account(&second);
    reward.tokens.push(TokenAmount {
        token: second,
        amount: 1_000_000,
    });
    let vault = vault_pda(&hash(&route, &reward)).0;
    let solver = ctx.solver.pubkey();
    let last = *reward.token_amounts().unwrap().keys().last().unwrap();
    ctx.airdrop(&vault, reward.native_amount).unwrap();
    for (mint, amount) in reward.token_amounts().unwrap() {
        let short = u64::from(mint == last);
        ctx.airdrop_token_ata(&mint, &vault, amount - short);
        ctx.airdrop_token_ata(&mint, &solver, 0);
    }
    prefund_solver(&mut ctx, &route);
    assert_reward_not_paid_in_full(&mut ctx, &route, &reward, true);
}

#[test]
fn rejects_vault_holding_one_entry_of_a_repeated_reward_mint() {
    let (mut ctx, route, mut reward) = unfunded_with_context(common::Context::default());
    let entry = reward.tokens[0].clone();
    reward.tokens.push(entry.clone());
    let vault = vault_pda(&hash(&route, &reward)).0;
    ctx.airdrop(&vault, reward.native_amount).unwrap();
    ctx.airdrop_token_ata(&entry.token, &vault, entry.amount);
    prefund_solver(&mut ctx, &route);
    assert_reward_not_paid_in_full(&mut ctx, &route, &reward, true);
}

#[test]
fn rejects_missing_vault_token_account() {
    let (mut ctx, route, reward) = unfunded_with_context(common::Context::default());
    let vault = vault_pda(&hash(&route, &reward)).0;
    ctx.airdrop(&vault, reward.native_amount).unwrap();
    prefund_solver(&mut ctx, &route);
    let instructions = pair(&ctx, &route, &reward);

    let failure = send(&mut ctx, &instructions).unwrap_err();

    // Portal's `InvalidTokenTransferAccounts`, raised while parsing the triples.
    assert_eq!(
        failure.err,
        TransactionError::InstructionError(
            2,
            InstructionError::Custom(u32::from(PortalError::InvalidTokenTransferAccounts)),
        )
    );
    assert!(ctx
        .get_account(&Proof::pda(&hash(&route, &reward), &reward.prover).0)
        .is_none());
}

fn transfer_fee_reward(basis_points: u16) -> (common::Context, Route, Reward) {
    let mut ctx = common::Context::new_with_token_2022();
    let fee_mint = ctx.create_transfer_fee_mint(basis_points, u64::MAX);
    let (mut ctx, route, mut reward) = unfunded_with_context(ctx);
    reward.tokens[0].token = fee_mint;
    let vault = vault_pda(&hash(&route, &reward)).0;
    ctx.airdrop(&vault, reward.native_amount).unwrap();
    ctx.airdrop_token_ata(&fee_mint, &vault, reward.tokens[0].amount);
    let solver = ctx.solver.pubkey();
    ctx.airdrop_token_ata(&fee_mint, &solver, 0);
    prefund_solver(&mut ctx, &route);
    (ctx, route, reward)
}

#[test]
fn rejects_reward_mint_with_transfer_fee() {
    let (mut ctx, route, reward) = transfer_fee_reward(100);
    assert_reward_not_paid_in_full(&mut ctx, &route, &reward, false);
}

#[test]
fn settles_reward_mint_with_zero_transfer_fee() {
    let (mut ctx, route, reward) = transfer_fee_reward(0);
    let instructions = pair(&ctx, &route, &reward);

    send(&mut ctx, &instructions).unwrap();

    assert_eq!(
        ctx.token_balance_ata(&reward.tokens[0].token, &ctx.solver.pubkey()),
        reward.tokens[0].amount
    );
    assert_settled(&ctx, &route, &reward);
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
    assert_settled(&ctx, &route, &reward);
    // The settlement emptied the vault, so a replay stops at the reward check.
    ctx.expire_blockhash();
    let failure = send(&mut ctx, &instructions).unwrap_err();
    assert_eq!(
        failure.err,
        TransactionError::InstructionError(
            2,
            InstructionError::Custom(u32::from(FlashFulfillerError::RewardNotPaidInFull)),
        )
    );
    // Stocked again, the replay's prove leg fails while the proof is open.
    let vault = vault_pda(&hash(&route, &reward)).0;
    ctx.airdrop(&vault, reward.native_amount).unwrap();
    ctx.airdrop_token_ata(&reward.tokens[0].token, &vault, reward.tokens[0].amount);
    ctx.expire_blockhash();
    let failure = send(&mut ctx, &instructions).unwrap_err();
    assert_eq!(
        failure.err,
        TransactionError::InstructionError(
            2,
            InstructionError::Custom(u32::from(LocalProverError::IntentAlreadyProven)),
        )
    );
    assert_settled(&ctx, &route, &reward);
    // After cleanup the replay proves again; portal's withdrawn marker rejects it.
    close_proof(&mut ctx, &route, &reward);
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
    let hash = hash(&route, &reward);
    assert!(ctx.get_account(&WithdrawnMarker::pda(&hash).0).is_some());
    assert!(ctx
        .get_account(&Proof::pda(&hash, &reward.prover).0)
        .is_none());
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

/// One venue route's intent. The relay stands in for the engine frame, so the
/// old flash shape puts SPL Token at depth 6. Only the venue's accounts are
/// committed: the relay forwards every account it receives to the venue, so
/// its own program account rides as an uncommitted trailing fulfill account.
fn venue_intent(
    ctx: &mut common::Context,
    venue: &VenueRoute,
    engine: &Instruction,
) -> (Route, Reward, Vec<AccountMeta>) {
    let executor = executor_pda().0;
    // Portal signs for the executor; the committed meta records no signature.
    let committed: Vec<_> = engine
        .accounts
        .iter()
        .cloned()
        .map(|mut meta| {
            meta.is_signer &= meta.pubkey != executor;
            meta
        })
        .collect();
    let calldata = Calldata {
        data: engine.data.clone(),
        account_count: committed.len() as u8,
    };
    let now = ctx.now();
    let route = Route {
        salt: rand::random::<[u8; 32]>().into(),
        deadline: now + 1800,
        portal: portal::ID.to_bytes().into(),
        native_amount: 0,
        tokens: vec![TokenAmount {
            token: venue.input_mint,
            amount: venue.amount_in,
        }],
        calls: vec![Call {
            target: engine.program_id.to_bytes().into(),
            data: borsh::to_vec(&CalldataWithAccounts::new(calldata, committed.clone()).unwrap())
                .unwrap(),
        }],
    };
    // The reward funds the route and leaves the solver a 10% spread.
    let reward = Reward {
        deadline: now + 3600,
        creator: ctx.creator.pubkey(),
        prover: local_prover::ID,
        native_amount: 0,
        tokens: vec![TokenAmount {
            token: venue.input_mint,
            amount: venue.amount_in + venue.amount_in / 10,
        }],
    };
    let funder = ctx.funder.pubkey();
    let vault = vault_pda(&hash(&route, &reward)).0;
    ctx.airdrop(&funder, common::sol_amount(1.0)).unwrap();
    ctx.set_token_balance(&funder, &venue.input_mint, reward.tokens[0].amount);
    let transfers = ctx.token_transfer_metas(funder, vault, venue.input_mint);
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
    let mut call_accounts = committed;
    call_accounts.push(AccountMeta::new_readonly(engine.program_id, false));
    (route, reward, call_accounts)
}

/// The same committed route through real venue and sub-program binaries: the
/// old flash shape fails with CallDepth; the split pair settles it inside one
/// client-shaped v0 packet that creates the solver's and the recipient's ATAs
/// and resolves a lookup table created before it; a second transaction then
/// closes the proof.
fn split_venue_route_fits_one_packet_and_delivers(
    build: impl FnOnce(&mut common::Context, &Pubkey) -> VenueRoute,
) {
    let mut ctx = common::Context::default().with_five_frame_limit();
    ctx.warp_to_timestamp(VENUE_SNAPSHOT_UNIX_TIMESTAMP);
    ctx.load_venue_mints();
    let recipient = Pubkey::new_unique();
    let venue = build(&mut ctx, &recipient);
    let engine = relay(&mut ctx, venue.instruction.clone());
    let (route, reward, call_accounts) = venue_intent(&mut ctx, &venue, &engine);
    let solver = ctx.solver.pubkey();
    let solver_ata = spl_ata(&solver, &venue.input_mint);
    let recipient_ata = spl_ata(&recipient, &venue.output_mint);

    // flash_fulfill[1] -> portal[2] -> engine[3] -> venue[4] -> sub-program[5]
    // -> SPL Token[6]. The venue reaches its deepest CPI only with an
    // initialized output account, so this attempt gets one.
    ctx.set_token_balance(&recipient, &venue.output_mint, 0);
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
    // The client transaction creates the recipient's ATA itself.
    ctx.set_account(recipient_ata, solana_sdk::account::Account::default())
        .unwrap();

    let table = ctx.create_venue_lookup_table(split_venue_lookup_addresses(
        &venue,
        &engine.program_id,
        &solver,
    ));
    let intent = hash(&route, &reward);
    let vault = vault_pda(&intent).0;
    for address in [
        ctx.payer.pubkey(),
        solver,
        recipient,
        recipient_ata,
        vault,
        spl_ata(&vault, &venue.input_mint),
        Proof::pda(&intent, &reward.prover).0,
        WithdrawnMarker::pda(&intent).0,
        FulfillMarker::pda(&intent).0,
    ] {
        assert!(!table.addresses.contains(&address), "{address} in table");
    }
    for ata in [solver_ata, recipient_ata] {
        assert!(ctx.get_account(&ata).is_none(), "{ata} exists");
    }
    let pair = [
        ctx.build_prove_and_withdraw_instruction(route.hash(), &reward),
        ctx.build_paired_fulfill_instruction(&route, &reward, call_accounts),
    ];
    // Proof cleanup cannot ride along: it would overflow the packet.
    let payer = ctx.payer.pubkey();
    let cleanup = ctx.portal().close_proof_instruction(
        CHAIN_ID,
        route.hash(),
        reward.clone(),
        vec![
            AccountMeta::new(Proof::pda(&intent, &reward.prover).0, false),
            AccountMeta::new(payer, true),
        ],
    );
    let with_cleanup = ctx.split_venue_transaction(
        &venue,
        &recipient,
        &[pair[0].clone(), pair[1].clone(), cleanup],
        table.clone(),
    );
    let measured = ctx.split_venue_transaction(&venue, &recipient, &pair, table);
    println!(
        "{}: {} bytes ({} headroom of {PACKET_DATA_SIZE}), {} account locks; {} bytes with \
         close_proof",
        venue.name,
        measured.wire_bytes,
        PACKET_DATA_SIZE as i64 - measured.wire_bytes as i64,
        measured.account_locks,
        with_cleanup.wire_bytes,
    );
    assert!(measured.wire_bytes <= PACKET_DATA_SIZE);
    assert!(measured.account_locks <= MAX_TX_ACCOUNT_LOCKS);
    assert!(with_cleanup.wire_bytes > PACKET_DATA_SIZE);

    let result = ctx.send_transaction(measured.transaction).unwrap();

    println!(
        "{}: {} CU of {SPLIT_VENUE_COMPUTE_LIMIT}",
        venue.name, result.compute_units_consumed
    );
    assert!(result.compute_units_consumed < u64::from(SPLIT_VENUE_COMPUTE_LIMIT));
    assert!(result
        .logs
        .contains(&format!("Program {} invoke [4]", venue.sub_program)));
    assert!(result
        .logs
        .contains(&format!("Program {} invoke [5]", spl_token::ID)));
    let delivered = ctx.token_balance(&recipient_ata);
    match venue.exact_out {
        Some(exact) => assert_eq!(delivered, exact),
        None => assert!(
            delivered >= venue.min_out,
            "{delivered} < {}",
            venue.min_out
        ),
    }
    assert_eq!(
        ctx.token_balance(&solver_ata),
        reward.tokens[0].amount - venue.amount_in
    );
    assert_eq!(
        ctx.token_balance(&spl_ata(&executor_pda().0, &venue.input_mint)),
        0
    );
    assert_settled(&ctx, &route, &reward);

    // The payer reclaims the proof's rent in its own transaction.
    let before = ctx.balance(&payer);
    close_proof(&mut ctx, &route, &reward);
    assert!(ctx.balance(&payer) > before);
}

#[test]
fn helium_treasury_route_fits_one_packet_and_settles_only_through_split() {
    split_venue_route_fits_one_packet_and_delivers(|ctx, recipient| {
        ctx.helium_treasury_route(recipient)
    });
}

#[test]
#[ignore = "needs SPLIT_VENUE_PROGRAMS; see tests/fixtures/split-venues/README.md"]
fn stabble_stable_swap_route_fits_one_packet_and_settles_only_through_split() {
    let programs = venue_programs_dir();
    split_venue_route_fits_one_packet_and_delivers(|ctx, recipient| {
        ctx.stabble_stable_swap_route(&programs, recipient)
    });
}

#[test]
#[ignore = "needs SPLIT_VENUE_PROGRAMS; see tests/fixtures/split-venues/README.md"]
fn meteora_damm_v1_stable_route_fits_one_packet_and_settles_only_through_split() {
    let programs = venue_programs_dir();
    split_venue_route_fits_one_packet_and_delivers(|ctx, recipient| {
        ctx.meteora_damm_v1_stable_route(&programs, recipient)
    });
}

#[test]
fn vendored_helium_binaries_match_their_dump_receipts() {
    assert_eq!(
        sha256_hex(HELIUM_TREASURY_MANAGEMENT_BIN),
        HELIUM_TREASURY_MANAGEMENT_SHA256
    );
    assert_eq!(
        sha256_hex(HELIUM_CIRCUIT_BREAKER_BIN),
        HELIUM_CIRCUIT_BREAKER_SHA256
    );
}
