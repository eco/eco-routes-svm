use anchor_lang::prelude::{borsh, AccountMeta};
use anchor_lang::solana_program::system_instruction;
use anchor_lang::{system_program, InstructionData};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::associated_token::spl_associated_token_account::instruction::create_associated_token_account;
use anchor_spl::token::spl_token;
use anchor_spl::token_2022::spl_token_2022;
use eco_svm_std::prover::{IntentHashClaimant, ProofData, ProveArgs};
use eco_svm_std::CHAIN_ID;
use hyper_prover::instructions::HyperProverError;
use portal::events::IntentFulfilled;
use portal::instructions::PortalError;
use portal::state::FulfillMarker;
use portal::types::{Call, Calldata, CalldataWithAccounts, Route};
use portal::{state, types};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

pub mod common;

fn route_with_calldatas(mut route: Route, calldatas: Vec<(Pubkey, Calldata)>) -> Route {
    route.calls = calldatas
        .into_iter()
        .map(|(target, calldata)| Call {
            target: target.to_bytes().into(),
            data: borsh::to_vec(&calldata).unwrap(),
        })
        .collect();

    route
}

fn route_with_calldatas_with_accounts(
    mut route: Route,
    calldatas_with_accounts: Vec<(Pubkey, CalldataWithAccounts)>,
) -> Route {
    route.calls = calldatas_with_accounts
        .into_iter()
        .map(|(target, calldata_with_accounts)| Call {
            target: target.to_bytes().into(),
            data: borsh::to_vec(&calldata_with_accounts).unwrap(),
        })
        .collect();

    route
}

#[test]
fn fulfill_intent_token_transfer_success() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.native_amount = 0;
    let reward_hash = rand::random::<[u8; 32]>().into();
    let token_program = &ctx.token_program.clone();
    let recipient = Pubkey::new_unique();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let solver = ctx.solver.pubkey();

    let (calldatas, call_accounts): (Vec<_>, Vec<_>) = route
        .tokens
        .iter()
        .map(|token| {
            let executor_ata = get_associated_token_address_with_program_id(
                &state::executor_pda().0,
                &token.token,
                token_program,
            );
            let recipient_ata = get_associated_token_address_with_program_id(
                &recipient,
                &token.token,
                token_program,
            );
            let calldata = Calldata {
                data: spl_token::instruction::transfer_checked(
                    token_program,
                    &executor_ata,
                    &token.token,
                    &recipient_ata,
                    &state::executor_pda().0,
                    &[],
                    token.amount,
                    6,
                )
                .unwrap()
                .data,
                account_count: 4,
            };
            let call_accounts = vec![
                AccountMeta::new(executor_ata, false),
                AccountMeta::new_readonly(token.token, false),
                AccountMeta::new(recipient_ata, false),
                AccountMeta::new_readonly(executor, false),
            ];

            (calldata, call_accounts)
        })
        .unzip();
    let calldatas_with_accounts: Vec<_> = calldatas
        .iter()
        .zip(call_accounts.iter())
        .map(|(calldata, call_accounts)| {
            CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap()
        })
        .collect();

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        calldatas_with_accounts
            .into_iter()
            .map(|calldata_with_accounts| (*token_program, calldata_with_accounts))
            .collect(),
    );
    let destination_route = route_with_calldatas(
        route,
        calldatas
            .into_iter()
            .map(|calldata| (*token_program, calldata))
            .collect(),
    );
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let (fulfill_marker, bump) = state::FulfillMarker::pda(&intent_hash);

    destination_route.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &solver, token.amount);
        ctx.airdrop_token_ata(&token.token, &recipient, 0);
    });
    let token_accounts: Vec<_> = destination_route
        .tokens
        .iter()
        .flat_map(|token| {
            let solver_ata =
                get_associated_token_address_with_program_id(&solver, &token.token, token_program);
            let executor_ata = get_associated_token_address_with_program_id(
                &executor,
                &token.token,
                token_program,
            );

            vec![
                AccountMeta::new(solver_ata, false),
                AccountMeta::new(executor_ata, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        token_accounts,
        call_accounts.into_iter().flatten(),
    );
    assert!(
        result.is_ok_and(common::contains_event(IntentFulfilled::new(
            intent_hash,
            claimant
        )))
    );
    destination_route.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &solver), 0);
        assert_eq!(ctx.token_balance_ata(&token.token, &executor), 0);
        assert_eq!(
            ctx.token_balance_ata(&token.token, &recipient),
            token.amount
        );
    });
    assert_eq!(
        ctx.account::<FulfillMarker>(&fulfill_marker).unwrap(),
        FulfillMarker::new(
            claimant,
            ctx.payer.pubkey(),
            destination_route.deadline,
            bump
        )
    );
}

#[test]
fn fulfill_intent_token_2022_transfer_success() {
    let mut ctx = common::Context::new_with_token_2022();
    let (_, mut route, _) = ctx.rand_intent();
    route.native_amount = 0;
    let reward_hash = rand::random::<[u8; 32]>().into();
    let token_program = &ctx.token_program.clone();
    let recipient = Pubkey::new_unique();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let solver = ctx.solver.pubkey();

    let (calldatas, call_accounts): (Vec<_>, Vec<_>) = route
        .tokens
        .iter()
        .map(|token| {
            let executor_ata = get_associated_token_address_with_program_id(
                &state::executor_pda().0,
                &token.token,
                token_program,
            );
            let recipient_ata = get_associated_token_address_with_program_id(
                &recipient,
                &token.token,
                token_program,
            );
            let calldata = Calldata {
                data: spl_token_2022::instruction::transfer_checked(
                    token_program,
                    &executor_ata,
                    &token.token,
                    &recipient_ata,
                    &state::executor_pda().0,
                    &[],
                    token.amount,
                    6,
                )
                .unwrap()
                .data,
                account_count: 4,
            };
            let call_accounts = vec![
                AccountMeta::new(executor_ata, false),
                AccountMeta::new_readonly(token.token, false),
                AccountMeta::new(recipient_ata, false),
                AccountMeta::new_readonly(executor, false),
            ];

            (calldata, call_accounts)
        })
        .unzip();
    let calldatas_with_accounts: Vec<_> = calldatas
        .iter()
        .zip(call_accounts.iter())
        .map(|(calldata, call_accounts)| {
            CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap()
        })
        .collect();

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        calldatas_with_accounts
            .into_iter()
            .map(|calldata_with_accounts| (*token_program, calldata_with_accounts))
            .collect(),
    );
    let destination_route = route_with_calldatas(
        route,
        calldatas
            .into_iter()
            .map(|calldata| (*token_program, calldata))
            .collect(),
    );
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let (fulfill_marker, bump) = state::FulfillMarker::pda(&intent_hash);

    destination_route.tokens.iter().for_each(|token| {
        ctx.airdrop_token_ata(&token.token, &solver, token.amount);
        ctx.airdrop_token_ata(&token.token, &recipient, 0);
    });
    let token_accounts: Vec<_> = destination_route
        .tokens
        .iter()
        .flat_map(|token| {
            let solver_ata =
                get_associated_token_address_with_program_id(&solver, &token.token, token_program);
            let executor_ata = get_associated_token_address_with_program_id(
                &executor,
                &token.token,
                token_program,
            );

            vec![
                AccountMeta::new(solver_ata, false),
                AccountMeta::new(executor_ata, false),
                AccountMeta::new_readonly(token.token, false),
            ]
        })
        .collect();

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        token_accounts,
        call_accounts.into_iter().flatten(),
    );
    assert!(
        result.is_ok_and(common::contains_event(IntentFulfilled::new(
            intent_hash,
            claimant
        )))
    );
    destination_route.tokens.iter().for_each(|token| {
        assert_eq!(ctx.token_balance_ata(&token.token, &solver), 0);
        assert_eq!(ctx.token_balance_ata(&token.token, &executor), 0);
        assert_eq!(
            ctx.token_balance_ata(&token.token, &recipient),
            token.amount
        );
    });
    assert_eq!(
        ctx.account::<FulfillMarker>(&fulfill_marker).unwrap(),
        FulfillMarker::new(
            claimant,
            ctx.payer.pubkey(),
            destination_route.deadline,
            bump
        )
    );
}

#[test]
fn fulfill_intent_native_transfer_success() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let recipient = Pubkey::new_unique();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let solver = ctx.solver.pubkey();

    ctx.airdrop(&solver, route.native_amount).unwrap();
    let calldata = Calldata {
        data: system_instruction::transfer(&executor, &recipient, route.native_amount).data,
        account_count: 3,
    };
    let call_accounts = vec![
        AccountMeta::new(executor, false),
        AccountMeta::new(recipient, false),
        AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
    ];
    let calldata_with_accounts =
        CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap();

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        vec![(system_program::ID, calldata_with_accounts)],
    );
    let destination_route =
        route_with_calldatas(route.clone(), vec![(system_program::ID, calldata)]);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let (fulfill_marker, bump) = state::FulfillMarker::pda(&intent_hash);

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        call_accounts,
    );
    assert!(
        result.is_ok_and(common::contains_event(IntentFulfilled::new(
            intent_hash,
            claimant
        )))
    );
    assert_eq!(ctx.balance(&solver), 0);
    assert_eq!(ctx.balance(&executor), 0);
    assert_eq!(ctx.balance(&recipient), route.native_amount);
    assert_eq!(
        ctx.account::<FulfillMarker>(&fulfill_marker).unwrap(),
        FulfillMarker::new(claimant, ctx.payer.pubkey(), route.deadline, bump)
    );
}

#[test]
fn fulfill_intent_executor_owner_reassigned_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.native_amount = 0;
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;

    // Give the executor a real System-owned account for the malicious route to target.
    ctx.airdrop(&executor, 10_000_000).unwrap();

    // Malicious route reassigns the executor's owner via the System Program.
    let attacker_program = Pubkey::new_unique();
    let calldata = Calldata {
        data: system_instruction::assign(&executor, &attacker_program).data,
        account_count: 1,
    };
    let call_accounts = vec![AccountMeta::new(executor, false)];
    let calldata_with_accounts =
        CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap();

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        vec![(system_program::ID, calldata_with_accounts)],
    );
    let destination_route = route_with_calldatas(route, vec![(system_program::ID, calldata)]);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        call_accounts,
    );
    assert!(result.is_err_and(common::is_error(PortalError::ExecutorCorrupted)));
}

#[test]
fn fulfill_intent_executor_allocated_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.native_amount = 0;
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;

    // Enough lamports to stay rent-exempt after the allocation so `allocate` fully succeeds;
    // the owner stays System Program, so only the data-empty invariant catches this.
    ctx.airdrop(&executor, 10_000_000).unwrap();

    let calldata = Calldata {
        data: system_instruction::allocate(&executor, 8).data,
        account_count: 1,
    };
    let call_accounts = vec![AccountMeta::new(executor, false)];
    let calldata_with_accounts =
        CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap();

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        vec![(system_program::ID, calldata_with_accounts)],
    );
    let destination_route = route_with_calldatas(route, vec![(system_program::ID, calldata)]);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        call_accounts,
    );
    assert!(result.is_err_and(common::is_error(PortalError::ExecutorCorrupted)));
}

#[test]
fn fulfill_intent_executor_ata_authority_reassigned_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.native_amount = 0;
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let token_program = &ctx.token_program.clone();

    // A mint the malicious intent never declares as a route token, with a pre-existing
    // executor ATA as an honest fulfill would leave. This is the cross-mint sink: the
    // executor is force-signed for the call, so the route can reassign the ATA's authority
    // even though the mint is absent from route.tokens.
    let victim_mint = Pubkey::new_unique();
    ctx.set_mint_account(&victim_mint);
    ctx.airdrop_token_ata(&victim_mint, &executor, 0);
    let victim_ata =
        get_associated_token_address_with_program_id(&executor, &victim_mint, token_program);

    let attacker = Pubkey::new_unique();
    let calldata = Calldata {
        data: spl_token::instruction::set_authority(
            token_program,
            &victim_ata,
            Some(&attacker),
            spl_token::instruction::AuthorityType::AccountOwner,
            &executor,
            &[],
        )
        .unwrap()
        .data,
        account_count: 2,
    };
    let call_accounts = vec![
        AccountMeta::new(victim_ata, false),
        AccountMeta::new_readonly(executor, false),
    ];
    let calldata_with_accounts =
        CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap();

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        vec![(*token_program, calldata_with_accounts)],
    );
    let destination_route = route_with_calldatas(route, vec![(*token_program, calldata)]);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        call_accounts,
    );
    assert!(result.is_err_and(common::is_error(PortalError::ExecutorAtaCorrupted)));
}

/// A delegate **persists** past the transaction, so installing one turns a right
/// that needs an intent authored and fulfilled into a standing one exercisable by
/// a bare transfer, over every future balance in the shared ATA.
#[test]
fn fulfill_intent_executor_ata_delegated_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.native_amount = 0;
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let token_program = &ctx.token_program.clone();

    let victim_mint = Pubkey::new_unique();
    ctx.set_mint_account(&victim_mint);
    ctx.airdrop_token_ata(&victim_mint, &executor, 0);
    let victim_ata =
        get_associated_token_address_with_program_id(&executor, &victim_mint, token_program);
    let attacker = Pubkey::new_unique();

    let calldata = Calldata {
        data: spl_token::instruction::approve(
            token_program,
            &victim_ata,
            &attacker,
            &executor,
            &[],
            u64::MAX,
        )
        .unwrap()
        .data,
        account_count: 3,
    };
    let call_accounts = vec![
        AccountMeta::new(victim_ata, false),
        AccountMeta::new_readonly(attacker, false),
        AccountMeta::new_readonly(executor, false),
    ];
    let calldata_with_accounts =
        CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap();

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        vec![(*token_program, calldata_with_accounts)],
    );
    let destination_route = route_with_calldatas(route, vec![(*token_program, calldata)]);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        call_accounts,
    );
    assert!(result.is_err_and(common::is_error(PortalError::ExecutorAtaCorrupted)));
}

/// `close_authority` persists too, and unlike the `AccountOwner` arm it carries no
/// `ImmutableOwner` guard, so it is reachable on token-2022 ATAs as well.
#[test]
fn fulfill_intent_executor_ata_close_authority_reassigned_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.native_amount = 0;
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let token_program = &ctx.token_program.clone();

    let victim_mint = Pubkey::new_unique();
    ctx.set_mint_account(&victim_mint);
    ctx.airdrop_token_ata(&victim_mint, &executor, 0);
    let victim_ata =
        get_associated_token_address_with_program_id(&executor, &victim_mint, token_program);
    let attacker = Pubkey::new_unique();

    let calldata = Calldata {
        data: spl_token::instruction::set_authority(
            token_program,
            &victim_ata,
            Some(&attacker),
            spl_token::instruction::AuthorityType::CloseAccount,
            &executor,
            &[],
        )
        .unwrap()
        .data,
        account_count: 2,
    };
    let call_accounts = vec![
        AccountMeta::new(victim_ata, false),
        AccountMeta::new_readonly(executor, false),
    ];
    let calldata_with_accounts =
        CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap();

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        vec![(*token_program, calldata_with_accounts)],
    );
    let destination_route = route_with_calldatas(route, vec![(*token_program, calldata)]);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        call_accounts,
    );
    assert!(result.is_err_and(common::is_error(PortalError::ExecutorAtaCorrupted)));
}

/// A closed ATA stops parsing entirely, which a post-call-only read could not
/// distinguish from an account that was never a token account.
#[test]
fn fulfill_intent_executor_ata_closed_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.native_amount = 0;
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let token_program = &ctx.token_program.clone();

    let victim_mint = Pubkey::new_unique();
    ctx.set_mint_account(&victim_mint);
    ctx.airdrop_token_ata(&victim_mint, &executor, 0);
    let victim_ata =
        get_associated_token_address_with_program_id(&executor, &victim_mint, token_program);
    let attacker = Pubkey::new_unique();

    let calldata = Calldata {
        data: spl_token::instruction::close_account(
            token_program,
            &victim_ata,
            &attacker,
            &executor,
            &[],
        )
        .unwrap()
        .data,
        account_count: 3,
    };
    let call_accounts = vec![
        AccountMeta::new(victim_ata, false),
        AccountMeta::new(attacker, false),
        AccountMeta::new_readonly(executor, false),
    ];
    let calldata_with_accounts =
        CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap();

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        vec![(*token_program, calldata_with_accounts)],
    );
    let destination_route = route_with_calldatas(route, vec![(*token_program, calldata)]);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        call_accounts,
    );
    assert!(result.is_err_and(common::is_error(PortalError::ExecutorAtaCorrupted)));
}

/// Executor ATAs must exist before `fulfill`, not be created by a route call.
///
/// A route-created one joins the digest only on the second pass, so the two
/// differ and the fulfill is rejected. The shape a multi-hop route needs — an
/// executor ATA for an intermediate mint that `route.tokens` never declares —
/// is reached instead by creating it beforehand: a permissionless
/// `create_idempotent` ahead of the `fulfill` instruction, or declaring the mint
/// in `route.tokens` with `amount: 0`. This pins the constraint as deliberate.
#[test]
fn fulfill_intent_route_created_executor_ata_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.native_amount = 0;
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let token_program = &ctx.token_program.clone();

    // a mint the route never declares, so `fund_executor` creates nothing for it
    let staged_mint = Pubkey::new_unique();
    ctx.set_mint_account(&staged_mint);
    let staged_ata =
        get_associated_token_address_with_program_id(&executor, &staged_mint, token_program);
    // the executor funds the creation itself, as a route call would arrange
    ctx.airdrop(&executor, common::sol_amount(1.0)).unwrap();

    let create = create_associated_token_account(&executor, &executor, &staged_mint, token_program);
    let calldata = Calldata {
        data: create.data,
        account_count: 6,
    };
    let call_accounts = vec![
        AccountMeta::new(executor, false),
        AccountMeta::new(staged_ata, false),
        AccountMeta::new_readonly(executor, false),
        AccountMeta::new_readonly(staged_mint, false),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(*token_program, false),
    ];
    let calldata_with_accounts =
        CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap();

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        vec![(anchor_spl::associated_token::ID, calldata_with_accounts)],
    );
    let destination_route =
        route_with_calldatas(route, vec![(anchor_spl::associated_token::ID, calldata)]);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        call_accounts,
    );
    assert!(result.is_err_and(common::is_error(PortalError::ExecutorAtaCorrupted)));
}

#[test]
fn fulfill_intent_executor_ata_reallocated_for_memo_fail() {
    let mut ctx = common::Context::new_with_token_2022();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.native_amount = 0;
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let token_program = &ctx.token_program.clone();

    // Same cross-mint sink as the authority-reassignment case, but the corruption
    // is an account-level extension rather than a base field: `mint` and `owner`
    // stay intact while every later transfer into the ATA is refused.
    //
    // Named for the reallocation because that is what this pins: the enable needs
    // the extension allocated first. Both calls are kept so the sequence is the
    // realistic one.
    let victim_mint = Pubkey::new_unique();
    ctx.set_mint_account(&victim_mint);
    ctx.airdrop_token_ata(&victim_mint, &executor, 0);
    // the executor pays its own reallocation, so the route needs no solver funds
    ctx.airdrop(&executor, common::sol_amount(1.0)).unwrap();
    let victim_ata =
        get_associated_token_address_with_program_id(&executor, &victim_mint, token_program);

    let reallocate = spl_token_2022::instruction::reallocate(
        token_program,
        &victim_ata,
        &executor,
        &executor,
        &[],
        &[spl_token_2022::extension::ExtensionType::MemoTransfer],
    )
    .unwrap();
    let enable_memo =
        spl_token_2022::extension::memo_transfer::instruction::enable_required_transfer_memos(
            token_program,
            &victim_ata,
            &executor,
            &[],
        )
        .unwrap();

    let reallocate_calldata = Calldata {
        data: reallocate.data,
        account_count: 4,
    };
    let enable_memo_calldata = Calldata {
        data: enable_memo.data,
        account_count: 2,
    };
    let call_accounts = vec![
        AccountMeta::new(victim_ata, false),
        AccountMeta::new(executor, false),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(executor, false),
        AccountMeta::new(victim_ata, false),
        AccountMeta::new_readonly(executor, false),
    ];

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        vec![
            (
                *token_program,
                CalldataWithAccounts::new(reallocate_calldata.clone(), call_accounts[..4].to_vec())
                    .unwrap(),
            ),
            (
                *token_program,
                CalldataWithAccounts::new(
                    enable_memo_calldata.clone(),
                    call_accounts[4..].to_vec(),
                )
                .unwrap(),
            ),
        ],
    );
    let destination_route = route_with_calldatas(
        route,
        vec![
            (*token_program, reallocate_calldata),
            (*token_program, enable_memo_calldata),
        ],
    );
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        call_accounts,
    );
    assert!(result.is_err_and(common::is_error(PortalError::ExecutorAtaCorrupted)));
}

#[test]
fn fulfill_intent_invalid_executor_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.calls.clear();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let wrong_executor = Pubkey::new_unique();

    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &route,
        reward_hash,
        claimant,
        wrong_executor,
        fulfill_marker,
        vec![],
        vec![],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidExecutor
    )));
}

#[test]
fn fulfill_intent_invalid_token_transfer_accounts_fail() {
    let mut ctx = common::Context::default();
    let (_, route, _) = ctx.rand_intent();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;

    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

    let insufficient_token_accounts = vec![AccountMeta::new(Pubkey::new_unique(), false)];

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        insufficient_token_accounts,
        vec![],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidTokenTransferAccounts
    )));
}

#[test]
fn fulfill_intent_invalid_mint_fail() {
    let mut ctx = common::Context::default();
    let (_, route, _) = ctx.rand_intent();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let solver = ctx.solver.pubkey();
    let token_program = &ctx.token_program.clone();

    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let fulfill_marker = state::FulfillMarker::pda(&intent_hash).0;

    let wrong_tokens: Vec<_> = (0..route.tokens.len())
        .map(|_| Pubkey::new_unique())
        .collect();

    wrong_tokens.iter().for_each(|wrong_token| {
        ctx.set_mint_account(wrong_token);
        ctx.airdrop_token_ata(wrong_token, &solver, 1_000_000);
    });

    let wrong_token_accounts: Vec<_> = wrong_tokens
        .iter()
        .flat_map(|wrong_token| {
            let solver_ata =
                get_associated_token_address_with_program_id(&solver, wrong_token, token_program);
            let executor_ata =
                get_associated_token_address_with_program_id(&executor, wrong_token, token_program);

            vec![
                AccountMeta::new(solver_ata, false),
                AccountMeta::new(executor_ata, false),
                AccountMeta::new_readonly(*wrong_token, false),
            ]
        })
        .collect();

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        wrong_token_accounts,
        vec![],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidMint
    )));
}

#[test]
fn fulfill_intent_invalid_fulfill_marker_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.native_amount = 0;
    route.tokens.clear();
    route.calls.clear();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;

    let wrong_fulfill_marker = Pubkey::new_unique();

    let result = ctx.portal().fulfill_intent(
        types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash),
        &route,
        reward_hash,
        claimant,
        executor,
        wrong_fulfill_marker,
        vec![],
        vec![],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidFulfillMarker
    )));
}

#[test]
fn fulfill_intent_invalid_calldata_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.native_amount = 0;
    route.tokens.clear();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let recipient = Pubkey::new_unique();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let native_amount = 1_000_000_000;

    ctx.airdrop(&executor, native_amount).unwrap();
    let calldata = Calldata {
        data: system_instruction::transfer(&executor, &recipient, native_amount).data,
        account_count: 3,
    };
    let call_accounts = vec![
        AccountMeta::new(executor, false),
        AccountMeta::new(recipient, false),
        AccountMeta::new_readonly(anchor_lang::system_program::ID, false),
    ];
    let calldata_with_accounts =
        CalldataWithAccounts::new(calldata.clone(), call_accounts.clone()).unwrap();

    let source_route = route_with_calldatas_with_accounts(
        route.clone(),
        vec![(system_program::ID, calldata_with_accounts)],
    );
    let destination_route = route_with_calldatas(route, vec![(system_program::ID, calldata)]);
    let intent_hash = types::intent_hash(CHAIN_ID, &source_route.hash(), &reward_hash);
    let (fulfill_marker, _) = state::FulfillMarker::pda(&intent_hash);

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &destination_route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        vec![call_accounts[0].clone(), call_accounts[1].clone()],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidCalldata
    )));
}

#[test]
fn fulfill_intent_already_fulfilled_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.native_amount = 0;
    route.tokens.clear();
    route.calls.clear();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
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

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        vec![],
    );
    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::IntentAlreadyFulfilled
    )));
}

#[test]
fn fulfill_intent_invalid_portal_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    route.calls.clear();
    route.portal = rand::random::<[u8; 32]>().into();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;

    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let (fulfill_marker, _) = state::FulfillMarker::pda(&intent_hash);

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        vec![],
    );

    assert!(result.is_err_and(common::is_error(
        portal::instructions::PortalError::InvalidPortal
    )));
}

#[test]
fn fulfill_intent_call_prover_with_executor_instead_of_dispatcher_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.native_amount = 0;
    route.tokens.clear();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let prove_data = ProveArgs {
        domain_id: 1,
        proof_data: ProofData::new(
            1,
            vec![IntentHashClaimant::new(
                rand::random::<[u8; 32]>().into(),
                rand::random::<[u8; 32]>().into(),
            )],
        ),
        data: rand::random::<[u8; 32]>().to_vec(),
    };
    let calldata = Calldata {
        data: hyper_prover::instruction::Prove { args: prove_data }.data(),
        account_count: 9,
    };
    let unique_message = solana_sdk::signature::Keypair::new();

    let call_accounts = vec![
        AccountMeta::new_readonly(executor, false),
        AccountMeta::new_readonly(hyper_prover::state::dispatcher_pda().0, false),
        AccountMeta::new(ctx.payer.pubkey(), false),
        AccountMeta::new(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(common::SPL_NOOP_ID, false),
        AccountMeta::new_readonly(unique_message.pubkey(), true),
        AccountMeta::new(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(hyper_prover::hyperlane::MAILBOX_ID, false),
        AccountMeta::new_readonly(hyper_prover::ID, false),
    ];
    let route_with_prover_call = route_with_calldatas(route, vec![(hyper_prover::ID, calldata)]);
    let intent_hash = types::intent_hash(CHAIN_ID, &route_with_prover_call.hash(), &reward_hash);
    let (fulfill_marker, _) = state::FulfillMarker::pda(&intent_hash);

    let result = ctx.portal().fulfill_intent_with_signers(
        intent_hash,
        &route_with_prover_call,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        call_accounts,
        vec![&unique_message],
    );
    assert!(result.is_err_and(common::is_error(HyperProverError::InvalidPortalDispatcher)));
}

#[test]
fn fulfill_intent_route_expired_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.tokens.clear();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;
    let intent_hash = types::intent_hash(CHAIN_ID, &route.hash(), &reward_hash);
    let (fulfill_marker, _) = state::FulfillMarker::pda(&intent_hash);

    ctx.warp_to_timestamp(1000);
    route.deadline = ctx.now() - 1;

    let result = ctx.portal().fulfill_intent(
        intent_hash,
        &route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        vec![],
    );
    assert!(result.is_err_and(common::is_error(PortalError::RouteExpired)));
}

#[test]
fn fulfill_intent_invalid_intent_hash_fail() {
    let mut ctx = common::Context::default();
    let (_, mut route, _) = ctx.rand_intent();
    route.native_amount = 0;
    route.tokens.clear();
    route.calls.clear();
    let reward_hash = rand::random::<[u8; 32]>().into();
    let claimant = Pubkey::new_unique().to_bytes().into();
    let executor = state::executor_pda().0;

    let wrong_intent_hash = rand::random::<[u8; 32]>().into();
    let fulfill_marker = state::FulfillMarker::pda(&wrong_intent_hash).0;

    let result = ctx.portal().fulfill_intent(
        wrong_intent_hash,
        &route,
        reward_hash,
        claimant,
        executor,
        fulfill_marker,
        vec![],
        vec![],
    );
    assert!(result.is_err_and(common::is_error(PortalError::InvalidIntentHash)));
}
