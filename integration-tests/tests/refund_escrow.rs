//! Timeout recovery is independent of child rendering, publication and fulfillment.
//! Each success invokes the compiled SBF program and asserts real token balances.

mod common;

use anchor_lang::InstructionData;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use common::intent_chainer_context::IDENTITY_SCALE;
use common::{
    contains_event, is_error, order_buffer_context as buffer, refund_escrow_context as refund,
    Context,
};
use intent_chainer::events::EscrowRefunded;
use intent_chainer::instructions::{ChainerError, RefundEscrowArgs};
use intent_chainer::state::OrderBuffer;
use intent_chainer::types::{
    Derivation, Order, SolanaDerivation, Template, TemplateProgram, Vault, MAX_ORDER_BYTES,
    MAX_ROUTE_LEN,
};
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;

const DEPOSIT: u64 = 1_234_567;

fn setup(token_2022: bool) -> (Context, Order, Pubkey) {
    let mut ctx = if token_2022 {
        Context::new_with_token_2022()
    } else {
        Context::default()
    };
    let mint = Pubkey::new_unique();
    ctx.set_mint_account(&mint);
    let destination_recipient = Pubkey::new_unique();
    let order = ctx.intent_chainer().svm_order(
        mint,
        destination_recipient,
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let recipient = prepare_recipient(&mut ctx, &order);
    (ctx, order, recipient)
}

fn prepare_recipient(ctx: &mut Context, order: &Order) -> Pubkey {
    ctx.airdrop_token_ata(&order.base_mint, &order.reward.creator, 0);
    get_associated_token_address_with_program_id(
        &order.reward.creator,
        &order.base_mint,
        &ctx.token_program,
    )
}

fn deposit(ctx: &mut Context, order: &Order, amount: u64) -> Pubkey {
    let (authority, escrow) = refund::escrow_accounts(ctx, order);
    ctx.airdrop_token_ata(&order.base_mint, &authority, amount);
    escrow
}

fn expire(ctx: &mut Context, order: &Order) {
    ctx.warp_to_timestamp(order.reward.deadline as i64);
}

fn event(order: &Order, amount: u64, amount_received: u64) -> EscrowRefunded {
    EscrowRefunded {
        order_commitment: order.hash(),
        escrow_authority: intent_chainer::state::escrow_authority_pda(&order.hash()).0,
        base_mint: order.base_mint,
        creator: order.reward.creator,
        amount,
        amount_received,
    }
}

#[test]
fn refund_rejects_before_deadline_and_succeeds_at_exact_boundary_without_creator_signature() {
    let (mut ctx, order, recipient) = setup(false);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    assert_ne!(ctx.payer.pubkey(), order.reward.creator);
    ctx.warp_to_timestamp(order.reward.deadline as i64 - 1);
    assert!(refund::refund(&mut ctx, &order, recipient)
        .is_err_and(is_error(ChainerError::RefundNotAvailable)));
    assert_eq!(ctx.token_balance(&escrow), DEPOSIT);
    assert_eq!(ctx.token_balance(&recipient), 0);

    expire(&mut ctx, &order);
    assert!(refund::refund(&mut ctx, &order, recipient)
        .is_ok_and(contains_event(event(&order, DEPOSIT, DEPOSIT))));
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
    assert!(ctx.get_account(&escrow).is_some(), "escrow stays open");
}

#[test]
fn permissionless_caller_cannot_select_creator_owned_alternate_with_third_party_delegate() {
    let (mut ctx, order, _) = setup(false);
    let recipient = Pubkey::new_unique();
    ctx.set_token_account(recipient, &order.base_mint, &order.reward.creator);
    let mut account = ctx.get_account(&recipient).unwrap();
    let mut token = anchor_spl::token::spl_token::state::Account::unpack(&account.data).unwrap();
    token.delegate = Some(ctx.solver.pubkey()).into();
    token.delegated_amount = u64::MAX;
    anchor_spl::token::spl_token::state::Account::pack(token, &mut account.data).unwrap();
    ctx.set_account(recipient, account).unwrap();
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    assert!(refund::refund(&mut ctx, &order, recipient)
        .is_err_and(is_error(ChainerError::CreatorSignatureRequired)));
    assert_eq!(ctx.token_balance(&escrow), DEPOSIT);
    assert_eq!(ctx.token_balance(&recipient), 0);
}

#[test]
fn creator_signature_can_authorize_alternate_owned_token_account() {
    let (mut ctx, order, _) = setup(false);
    let recipient = Pubkey::new_unique();
    ctx.set_token_account(recipient, &order.base_mint, &order.reward.creator);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    let creator = ctx.creator.insecure_clone();
    let instruction = refund::inline(&ctx, &order, recipient);
    refund::send_with_creator(&mut ctx, instruction, &creator).unwrap();
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
}

#[test]
fn unrelated_signer_cannot_authorize_alternate_creator_owned_account() {
    let (mut ctx, order, _) = setup(false);
    let recipient = Pubkey::new_unique();
    ctx.set_token_account(recipient, &order.base_mint, &order.reward.creator);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    let attacker = ctx.solver.insecure_clone();
    let mut instruction = refund::inline(&ctx, &order, recipient);
    instruction.accounts.last_mut().unwrap().pubkey = attacker.pubkey();
    assert!(refund::send_with_creator(&mut ctx, instruction, &attacker)
        .is_err_and(is_error(ChainerError::InvalidRefundRecipient)));
    assert_eq!(ctx.token_balance(&escrow), DEPOSIT);
    assert_eq!(ctx.token_balance(&recipient), 0);
}

#[test]
fn different_recipient_owner_cannot_redirect_refund() {
    let (mut ctx, order, recipient) = setup(false);
    let attacker = Pubkey::new_unique();
    ctx.airdrop_token_ata(&order.base_mint, &attacker, 0);
    let attacker_ata = get_associated_token_address_with_program_id(
        &attacker,
        &order.base_mint,
        &ctx.token_program,
    );
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    assert!(refund::refund(&mut ctx, &order, attacker_ata)
        .is_err_and(is_error(ChainerError::InvalidRefundRecipient)));
    assert_eq!(ctx.token_balance(&escrow), DEPOSIT);
    assert_eq!(ctx.token_balance(&attacker_ata), 0);
    assert_eq!(ctx.token_balance(&recipient), 0);
}

#[test]
fn recipient_with_wrong_mint_is_rejected_without_movement() {
    let (mut ctx, order, _) = setup(false);
    let other_mint = Pubkey::new_unique();
    ctx.set_mint_account(&other_mint);
    let recipient = Pubkey::new_unique();
    ctx.set_token_account(recipient, &other_mint, &order.reward.creator);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    assert!(
        refund::refund(&mut ctx, &order, recipient).is_err_and(is_error(ChainerError::InvalidMint))
    );
    assert_eq!(ctx.token_balance(&escrow), DEPOSIT);
    assert_eq!(ctx.token_balance(&recipient), 0);
}

#[test]
fn changed_creator_deadline_or_recipe_cannot_spend_original_escrow() {
    let (mut ctx, order, recipient) = setup(false);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    let mut variants = vec![order.clone(); 3];
    variants[0].reward.creator = Pubkey::new_unique();
    variants[1].reward.deadline -= 1;
    variants[2].scale += 1;
    for changed in variants {
        let mut instruction = refund::inline(&ctx, &order, recipient);
        instruction.data = intent_chainer::instruction::RefundEscrow {
            args: RefundEscrowArgs { order: changed },
        }
        .data();
        assert!(refund::send(&mut ctx, instruction)
            .is_err_and(is_error(ChainerError::InvalidEscrowAuthority)));
        assert_eq!(ctx.token_balance(&escrow), DEPOSIT);
        assert_eq!(ctx.token_balance(&recipient), 0);
    }
}

#[test]
fn substituted_base_mint_is_rejected_without_movement() {
    let (mut ctx, order, recipient) = setup(false);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    let other_mint = Pubkey::new_unique();
    ctx.set_mint_account(&other_mint);
    let mut instruction = refund::inline(&ctx, &order, recipient);
    instruction.accounts[2].pubkey = other_mint;
    expire(&mut ctx, &order);
    assert!(refund::send(&mut ctx, instruction).is_err_and(is_error(ChainerError::InvalidMint)));
    assert_eq!(ctx.token_balance(&escrow), DEPOSIT);
}

#[test]
fn noncanonical_escrow_token_account_is_rejected() {
    let (mut ctx, order, recipient) = setup(false);
    let (authority, _) = refund::escrow_accounts(&ctx, &order);
    let noncanonical = Pubkey::new_unique();
    ctx.set_token_account(noncanonical, &order.base_mint, &authority);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    let mut instruction = refund::inline(&ctx, &order, recipient);
    instruction.accounts[1].pubkey = noncanonical;
    expire(&mut ctx, &order);
    assert!(
        refund::send(&mut ctx, instruction).is_err_and(is_error(ChainerError::InvalidEscrowAta))
    );
    assert_eq!(ctx.token_balance(&escrow), DEPOSIT);
}

#[test]
fn canonical_escrow_address_with_wrong_token_owner_is_rejected() {
    let (mut ctx, order, recipient) = setup(false);
    let (_, escrow) = refund::escrow_accounts(&ctx, &order);
    // A deliberately malformed account pins owner validation independently of
    // address validation; this state is not produced by an honest ATA creation.
    ctx.set_token_account(escrow, &order.base_mint, &Pubkey::new_unique());
    expire(&mut ctx, &order);
    assert!(refund::refund(&mut ctx, &order, recipient)
        .is_err_and(is_error(ChainerError::InvalidEscrowTokenOwner)));
    assert_eq!(ctx.token_balance(&recipient), 0);
}

#[test]
fn refund_works_below_floor_with_zero_scale_invalid_template_and_unavailable_portal() {
    let (mut ctx, original, recipient) = setup(false);
    let mut orders = vec![original.clone(); 5];
    orders[0].min_amount_in = DEPOSIT + 1;
    orders[1].scale = 0;
    orders[2].template.route.segments.pop();
    orders[3].portal = Pubkey::new_unique();
    orders[3].require_publish = true;
    orders[4].scale = u128::MAX; // The normal u64 route amount cannot fit.
    expire(&mut ctx, &original);
    for (index, order) in orders.iter().enumerate() {
        let escrow = deposit(&mut ctx, order, DEPOSIT);
        refund::refund(&mut ctx, order, recipient)
            .unwrap_or_else(|error| panic!("recovery variant {index}: {error:?}"));
        assert_eq!(ctx.token_balance(&escrow), 0);
        assert_eq!(ctx.token_balance(&recipient), DEPOSIT * (index as u64 + 1));
    }
}

#[test]
fn malformed_child_reward_does_not_disable_escrow_refund() {
    let (mut ctx, mut order, recipient) = setup(false);
    order.reward.tokens.clear();
    order.reward.native_amount = 17;
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    refund::refund(&mut ctx, &order, recipient).unwrap();
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
}

#[test]
fn already_settled_child_does_not_prevent_refund_of_remaining_escrow() {
    let (mut ctx, order, recipient) = setup(false);
    let child = ctx
        .intent_chainer()
        .resolve(&order, DEPOSIT, Pubkey::new_unique());
    let marker = intent_chainer::state::withdrawn_marker_pda(&order.portal, &child.intent_hash).0;
    ctx.set_withdrawn_marker(marker);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    assert!(ctx
        .intent_chainer()
        .chain(&child, true)
        .is_err_and(is_error(ChainerError::IntentAlreadySettled)));
    refund::refund(&mut ctx, &order, recipient).unwrap();
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
    assert!(ctx.get_account(&marker).is_some());
}

#[test]
fn frozen_canonical_ata_preserves_escrow_until_creator_authorizes_alternate_account() {
    let (mut ctx, order, recipient) = setup(false);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    // Model a recipient frozen by the mint authority. Chainer's custody checks
    // still pass; the actual SPL transfer rejects and must not debit the escrow.
    let mut account = ctx.get_account(&recipient).unwrap();
    let mut token = anchor_spl::token::spl_token::state::Account::unpack(&account.data).unwrap();
    token.state = anchor_spl::token::spl_token::state::AccountState::Frozen;
    anchor_spl::token::spl_token::state::Account::pack(token, &mut account.data).unwrap();
    ctx.set_account(recipient, account).unwrap();
    expire(&mut ctx, &order);
    let result = refund::refund(&mut ctx, &order, recipient);
    assert!(result.is_err_and(is_error(
        anchor_spl::token::spl_token::error::TokenError::AccountFrozen as u32
    )));
    assert_eq!(ctx.token_balance(&escrow), DEPOSIT);
    assert_eq!(ctx.token_balance(&recipient), 0);

    let alternate = Pubkey::new_unique();
    ctx.set_token_account(alternate, &order.base_mint, &order.reward.creator);
    let creator = ctx.creator.insecure_clone();
    let instruction = refund::inline(&ctx, &order, alternate);
    refund::send_with_creator(&mut ctx, instruction, &creator).unwrap();
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), 0);
    assert_eq!(ctx.token_balance(&alternate), DEPOSIT);
}

#[test]
fn repeated_refund_is_idempotent_and_late_donations_remain_recoverable() {
    let (mut ctx, order, recipient) = setup(false);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    refund::refund(&mut ctx, &order, recipient).unwrap();
    assert!(
        refund::refund(&mut ctx, &order, recipient).is_ok_and(contains_event(event(&order, 0, 0)))
    );
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
    assert!(ctx.get_account(&escrow).is_some());

    deposit(&mut ctx, &order, 29);
    refund::refund(&mut ctx, &order, recipient).unwrap();
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT + 29);
}

#[test]
fn token_2022_refund_moves_the_full_balance() {
    let (mut ctx, order, recipient) = setup(true);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    refund::refund(&mut ctx, &order, recipient).unwrap();
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
}

#[test]
fn token_2022_transfer_fee_refund_reports_gross_debit_and_net_receipt() {
    let mut ctx = Context::new_with_token_2022();
    let mint = ctx.create_transfer_fee_mint(100, u64::MAX);
    let order = ctx.intent_chainer().svm_order(
        mint,
        Pubkey::new_unique(),
        local_prover::ID,
        IDENTITY_SCALE,
        1,
    );
    let recipient = prepare_recipient(&mut ctx, &order);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    let fee = DEPOSIT.div_ceil(100);
    assert!(
        refund::refund(&mut ctx, &order, recipient).is_ok_and(contains_event(event(
            &order,
            DEPOSIT,
            DEPOSIT - fee
        )))
    );
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT - fee);
}

#[test]
fn incomplete_buffer_cannot_refund_but_complete_unsealed_buffer_can() {
    let (mut ctx, order, recipient) = setup(false);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    let writer = ctx.funder.insecure_clone();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let seed = [71; 32];
    let bytes = borsh::to_vec(&order).unwrap();
    let first = bytes.len() / 2;
    buffer::send(
        &mut ctx,
        &writer,
        &[buffer::init(
            writer.pubkey(),
            seed,
            order.hash(),
            bytes.len(),
            &bytes[..first],
        )],
    )
    .0
    .unwrap();
    let address = OrderBuffer::pda(&writer.pubkey(), &seed).0;
    expire(&mut ctx, &order);
    let instruction = refund::buffered(&ctx, &order, recipient, address);
    assert!(refund::send(&mut ctx, instruction)
        .is_err_and(is_error(ChainerError::OrderBufferIncomplete)));
    assert_eq!(ctx.token_balance(&escrow), DEPOSIT);

    buffer::send(
        &mut ctx,
        &writer,
        &[buffer::write(
            writer.pubkey(),
            address,
            first as u32,
            &bytes[first..],
        )],
    )
    .0
    .unwrap();
    assert!(!ctx.account::<OrderBuffer>(&address).unwrap().sealed);
    // Only the unrelated fee payer signs the recovery instruction.
    let instruction = refund::buffered(&ctx, &order, recipient, address);
    refund::send(&mut ctx, instruction).unwrap();
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
    assert!(
        ctx.get_account(&address).is_some(),
        "refund does not close the buffer"
    );
}

#[test]
fn unsealable_large_order_is_refundable_through_native_buffer_packets() {
    let (mut ctx, initial, recipient) = setup(false);
    // Large canonical commitments use more than the default 400k CU. Match the
    // maximum nested-order tests while retaining Solana's stock 32KiB heap.
    ctx.compute_limit = 1_400_000;
    let mut order =
        ctx.intent_chainer()
            .large_route_order(initial.base_mint, local_prover::ID, MAX_ROUTE_LEN);
    order.scale = 0;
    order.portal = Pubkey::new_unique();
    order.require_publish = true;
    let bytes = borsh::to_vec(&order).unwrap();
    assert!(bytes.len() > common::template_transport::PACKET_BYTES);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    let writer = ctx.funder.insecure_clone();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let address = buffer::upload(&mut ctx, &writer, [72; 32], order.hash(), &bytes);
    assert!(
        buffer::send(&mut ctx, &writer, &[buffer::seal(writer.pubkey(), address)])
            .0
            .is_err_and(is_error(ChainerError::InvalidScale))
    );
    expire(&mut ctx, &order);
    let instruction = refund::buffered(&ctx, &order, recipient, address);
    let meta = refund::send(&mut ctx, instruction).unwrap();
    println!(
        "large unsealable refund: order={} CU={}",
        bytes.len(),
        meta.compute_units_consumed
    );
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
}

#[test]
fn exact_maximum_order_bytes_can_refund_without_rendering_or_custom_heap() {
    let (mut ctx, mut order, recipient) = setup(false);
    ctx.compute_limit = 1_400_000;
    // Individually bounded templates remain decodable, but their aggregate is
    // intentionally unrenderable. Recovery must not inherit that render limit.
    order.template = TemplateProgram {
        vaults: vec![Vault {
            destination: order.destination,
            route: Template::literal(vec![0x71; MAX_ROUTE_LEN]),
            reward: Template::literal(vec![]),
            derivation: Derivation::Solana(SolanaDerivation {
                portal: order.portal,
                token_program: ctx.token_program,
                mint: order.base_mint,
            }),
        }],
        route: Template::literal(vec![]),
    };
    let padding = MAX_ORDER_BYTES - borsh::to_vec(&order).unwrap().len();
    assert!(padding <= MAX_ROUTE_LEN);
    order.template.route.segments[0] = vec![0x72; padding];
    let bytes = borsh::to_vec(&order).unwrap();
    assert_eq!(bytes.len(), MAX_ORDER_BYTES);
    let decoded: Order = borsh::from_slice(&bytes).unwrap();
    assert_eq!(decoded.hash(), order.hash());

    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    let writer = ctx.funder.insecure_clone();
    ctx.airdrop(&writer.pubkey(), common::sol_amount(1.0))
        .unwrap();
    let address = buffer::upload(&mut ctx, &writer, [75; 32], order.hash(), &bytes);
    assert!(
        buffer::send(&mut ctx, &writer, &[buffer::seal(writer.pubkey(), address)])
            .0
            .is_err_and(is_error(ChainerError::RenderedBytesExceeded))
    );
    expire(&mut ctx, &order);
    let instruction = refund::buffered(&ctx, &order, recipient, address);
    let meta = refund::send(&mut ctx, instruction).unwrap();
    assert!(meta.compute_units_consumed <= 1_400_000);
    assert!(contains_event(event(&order, DEPOSIT, DEPOSIT))(
        meta.clone()
    ));
    println!(
        "maximum unsealable refund: order={} CU={}",
        bytes.len(),
        meta.compute_units_consumed
    );
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
}

#[test]
fn buffered_order_hash_must_match_header_and_funded_escrow() {
    let (mut ctx, order, recipient) = setup(false);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    let mut other_order = order.clone();
    other_order.scale += 1;
    let bytes = borsh::to_vec(&other_order).unwrap();
    let writer = ctx.payer.insecure_clone();
    let address = buffer::upload(&mut ctx, &writer, [73; 32], order.hash(), &bytes);
    expire(&mut ctx, &order);
    let instruction = refund::buffered(&ctx, &order, recipient, address);
    assert!(refund::send(&mut ctx, instruction)
        .is_err_and(is_error(ChainerError::OrderCommitmentMismatch)));
    assert_eq!(ctx.token_balance(&escrow), DEPOSIT);
    assert_eq!(ctx.token_balance(&recipient), 0);
}

#[test]
fn complete_buffer_with_trailing_bytes_is_not_a_canonical_order() {
    let (mut ctx, order, recipient) = setup(false);
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    let mut bytes = borsh::to_vec(&order).unwrap();
    bytes.push(0);
    let writer = ctx.payer.insecure_clone();
    let address = buffer::upload(&mut ctx, &writer, [74; 32], order.hash(), &bytes);
    expire(&mut ctx, &order);
    let instruction = refund::buffered(&ctx, &order, recipient, address);
    assert!(refund::send(&mut ctx, instruction)
        .is_err_and(is_error(ChainerError::InvalidBufferedOrder)));
    assert_eq!(ctx.token_balance(&escrow), DEPOSIT);
}

#[test]
fn refund_winning_race_prevents_chain_from_moving_the_same_deposit() {
    let (mut ctx, order, recipient) = setup(false);
    let child = ctx
        .intent_chainer()
        .resolve(&order, DEPOSIT, Pubkey::new_unique());
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    refund::refund(&mut ctx, &order, recipient).unwrap();
    assert!(ctx
        .intent_chainer()
        .chain(&child, true)
        .is_err_and(is_error(ChainerError::ZeroAmount)));
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
    assert_eq!(ctx.token_balance(&child.vault_ata), 0);
}

#[test]
fn chain_winning_race_leaves_refund_empty_and_portal_refund_pays_creator_once() {
    let (mut ctx, order, recipient) = setup(false);
    let child = ctx
        .intent_chainer()
        .resolve(&order, DEPOSIT, Pubkey::new_unique());
    let escrow = deposit(&mut ctx, &order, DEPOSIT);
    expire(&mut ctx, &order);
    ctx.intent_chainer().chain(&child, true).unwrap();
    refund::refund(&mut ctx, &order, recipient).unwrap();
    assert_eq!(ctx.token_balance(&escrow), 0);
    assert_eq!(ctx.token_balance(&recipient), 0);
    assert_eq!(ctx.token_balance(&child.vault_ata), DEPOSIT);

    // Late chain retains Portal's existing recovery semantics. Portal uses a
    // strict deadline comparison, so advance beyond it before refunding child.
    ctx.warp_to_timestamp(order.reward.deadline as i64 + 1);
    ctx.portal()
        .refund_intent(
            order.destination,
            child.reward.clone(),
            child.vault,
            child.route.hash(),
            eco_svm_std::prover::Proof::pda(&child.intent_hash, &order.reward.prover).0,
            intent_chainer::state::withdrawn_marker_pda(&order.portal, &child.intent_hash).0,
            order.reward.creator,
            vec![
                solana_sdk::instruction::AccountMeta::new(child.vault_ata, false),
                solana_sdk::instruction::AccountMeta::new(recipient, false),
                solana_sdk::instruction::AccountMeta::new_readonly(order.base_mint, false),
            ],
        )
        .unwrap();
    assert_eq!(ctx.token_balance(&child.vault_ata), 0);
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
    refund::refund(&mut ctx, &order, recipient).unwrap();
    assert_eq!(ctx.token_balance(&recipient), DEPOSIT);
}
