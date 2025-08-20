use anchor_lang::prelude::AccountMeta;
use anchor_lang::solana_program::system_instruction;
use anchor_lang::{system_program, AnchorSerialize, InstructionData};
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
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
            data: calldata.try_to_vec().unwrap(),
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
            data: calldata_with_accounts.try_to_vec().unwrap(),
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
        FulfillMarker::new(claimant, bump)
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
        FulfillMarker::new(claimant, bump)
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
        AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
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
        FulfillMarker::new(claimant, bump)
    );
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
        AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
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
        AccountMeta::new_readonly(spl_noop::ID, false),
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
