use anchor_lang::AnchorDeserialize;
use eco_svm_std::prover::{GetProofArgs, IntentHashClaimant, IntentProven, Proof, ProofData};
use eco_svm_std::CANCELLED;
use layerzero_prover::instructions::LayerZeroProverError;
use layerzero_prover::layerzero::{self, LzInstruction, LzReceiveTypesV2Result};
use layerzero_prover::state::{pda_payer_pda, ProofAccount, Store};
use portal::instructions::PortalError;
use portal::state::{vault_pda, WithdrawnMarker};
use solana_sdk::account::Account;
use solana_sdk::instruction::AccountMeta;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;

use crate::common::layerzero_prover_context::{
    cleanup_tail, evm_peer, peers, receive_params, BASE_CHAIN_ID, BASE_EID, OP_CHAIN_ID,
};

pub mod common;

fn ready() -> common::Context {
    let mut context = common::Context::default();
    context.layerzero_prover().setup();
    context
}

fn pairs(hashes: &[(u8, Pubkey)]) -> ProofData {
    ProofData::new(
        BASE_CHAIN_ID,
        hashes
            .iter()
            .map(|(byte, claimant)| {
                IntentHashClaimant::new([*byte; 32].into(), claimant.to_bytes().into())
            })
            .collect(),
    )
}

fn proof(context: &common::Context, byte: u8) -> Option<Proof> {
    context
        .account::<ProofAccount>(&Proof::pda(&[byte; 32].into(), &layerzero_prover::ID).0)
        .map(|account| account.0)
}

#[test]
fn delivers_and_creates_proofs() {
    let mut context = ready();
    let (alice, bob) = (Pubkey::new_unique(), Pubkey::new_unique());
    let params = receive_params(&peers()[0], 1, pairs(&[(1, alice), (2, bob)]));
    let payload_hash =
        layerzero::payload_hash_pda(&Store::pda().0, params.src_eid, &params.sender, 1).0;

    let result = context.layerzero_prover().deliver(&params).unwrap();

    assert_eq!(proof(&context, 1).unwrap().claimant, alice);
    assert_eq!(proof(&context, 2).unwrap().destination, BASE_CHAIN_ID);
    assert!(common::contains_cpi_event(IntentProven::new(
        [1; 32].into(),
        alice,
        BASE_CHAIN_ID
    ))(result));
    assert!(context.get_account(&payload_hash).is_none());
}

/// The executor builds `lz_receive` from `lz_receive_types_v2`'s answer.
#[test]
fn delivery_built_from_v2_discovery_succeeds() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    context.layerzero_prover().verify(&params).unwrap();
    let discovered = context
        .layerzero_prover()
        .lz_receive_types_v2(params.clone())
        .unwrap();
    let mut decoded = LzReceiveTypesV2Result::try_from_slice(&discovered.return_data.data).unwrap();
    let LzInstruction::LzReceive { accounts } = decoded.instructions.remove(0) else {
        panic!("expected an LzReceive instruction")
    };

    let instruction = context
        .layerzero_prover()
        .lz_receive_instruction(&params, accounts);
    assert!(context
        .layerzero_prover()
        .send(vec![instruction], &[])
        .is_ok());
    assert!(proof(&context, 1).is_some());
}

#[test]
fn replayed_delivery_fails() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    context.layerzero_prover().deliver(&params).unwrap();
    context.expire_blockhash();

    let result = context.layerzero_prover().lz_receive(&params);

    // The first delivery's `clear` closed the PayloadHash, so the endpoint
    // rejects the replay before `lz_receive` changes any state.
    assert!(result.is_err_and(common::is_error(
        anchor_lang::error::ErrorCode::AccountNotInitialized
    )));
}

#[test]
fn chain_id_mismatch_rejected() {
    let mut context = ready();
    let mut data = pairs(&[(1, Pubkey::new_unique())]);
    data.destination = OP_CHAIN_ID;
    let params = receive_params(&peers()[0], 1, data);

    let result = context.layerzero_prover().deliver(&params);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::ChainIdMismatch)));
    assert!(proof(&context, 1).is_none());
}

#[test]
fn unknown_src_eid_rejected() {
    let mut context = ready();
    let stranger = evm_peer(40_245, BASE_CHAIN_ID, 0xba);
    context
        .layerzero_prover()
        .force_nonce(stranger.eid, stranger.address.into());
    let params = receive_params(&stranger, 1, pairs(&[(1, Pubkey::new_unique())]));

    let result = context.layerzero_prover().deliver(&params);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::UnknownPeer)));
    assert!(proof(&context, 1).is_none());
}

/// A griefer pre-funding a `Proof` PDA cannot block its creation.
#[test]
fn prefunded_proof_pda_still_created() {
    let mut context = ready();
    let alice = Pubkey::new_unique();
    context
        .airdrop(
            &Proof::pda(&[1; 32].into(), &layerzero_prover::ID).0,
            1_000_000,
        )
        .unwrap();

    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, pairs(&[(1, alice)])))
        .unwrap();

    let proof = proof(&context, 1).unwrap();
    assert_eq!(proof.claimant, alice);
    assert_eq!(proof.destination, BASE_CHAIN_ID);
}

#[test]
fn non_peer_sender_rejected_even_if_endpoint_path_exists() {
    let mut context = ready();
    let stranger = evm_peer(BASE_EID, BASE_CHAIN_ID, 0x66);
    context
        .layerzero_prover()
        .force_nonce(stranger.eid, stranger.address.into());
    let params = receive_params(&stranger, 1, pairs(&[(1, Pubkey::new_unique())]));

    let result = context.layerzero_prover().deliver(&params);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidSender)));
}

#[test]
fn wrong_receiver_in_clear_accounts_rejected() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    context.layerzero_prover().verify(&params).unwrap();
    let mut accounts = layerzero_prover::instructions::lz_receive_accounts(
        &params,
        &ProofData::from_bytes(&params.message).unwrap(),
    );
    accounts[6].pubkey = layerzero::AddressLocator::Address(Pubkey::new_unique());

    let instruction = context
        .layerzero_prover()
        .lz_receive_instruction(&params, accounts);
    let result = context.layerzero_prover().send(vec![instruction], &[]);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidStore)));
}

#[test]
fn proof_account_mismatch_rejected() {
    let mut context = ready();
    let params = receive_params(
        &peers()[0],
        1,
        pairs(&[(1, Pubkey::new_unique()), (2, Pubkey::new_unique())]),
    );
    context.layerzero_prover().verify(&params).unwrap();
    let mut accounts = layerzero_prover::instructions::lz_receive_accounts(
        &params,
        &ProofData::from_bytes(&params.message).unwrap(),
    );
    accounts.pop();

    let instruction = context
        .layerzero_prover()
        .lz_receive_instruction(&params, accounts);
    let result = context.layerzero_prover().send(vec![instruction], &[]);

    assert!(result.is_err_and(common::is_error(LayerZeroProverError::InvalidProof)));
}

#[test]
fn redelivery_is_idempotent_and_conflict_fails() {
    let mut context = ready();
    let alice = Pubkey::new_unique();
    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, pairs(&[(1, alice)])))
        .unwrap();

    let same =
        context
            .layerzero_prover()
            .deliver(&receive_params(&peers()[0], 2, pairs(&[(1, alice)])));
    assert!(same.is_ok());

    let conflict = context.layerzero_prover().deliver(&receive_params(
        &peers()[0],
        3,
        pairs(&[(1, Pubkey::new_unique())]),
    ));
    assert!(conflict.is_err_and(common::is_error(LayerZeroProverError::IntentAlreadyProven)));
    assert_eq!(proof(&context, 1).unwrap().claimant, alice);
}

#[test]
fn duplicate_pair_in_one_message_is_idempotent_and_conflict_fails() {
    let mut context = ready();
    let alice = Pubkey::new_unique();

    let same = context.layerzero_prover().deliver(&receive_params(
        &peers()[0],
        1,
        pairs(&[(1, alice), (1, alice)]),
    ));
    assert!(same.is_ok());

    let conflict = context.layerzero_prover().deliver(&receive_params(
        &peers()[0],
        2,
        pairs(&[(2, alice), (2, Pubkey::new_unique())]),
    ));
    assert!(conflict.is_err_and(common::is_error(LayerZeroProverError::IntentAlreadyProven)));
    assert!(proof(&context, 2).is_none());
}

#[test]
fn underfunded_pda_payer_fails_then_retry_succeeds() {
    let mut context = ready();
    let params = receive_params(&peers()[0], 1, pairs(&[(1, Pubkey::new_unique())]));
    let payload_hash =
        layerzero::payload_hash_pda(&Store::pda().0, params.src_eid, &params.sender, 1).0;
    context.layerzero_prover().verify(&params).unwrap();
    context
        .set_account(
            pda_payer_pda().0,
            Account {
                lamports: 0,
                data: vec![],
                owner: anchor_lang::system_program::ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();

    assert!(context.layerzero_prover().lz_receive(&params).is_err());
    assert!(context.get_account(&payload_hash).is_some());

    context.airdrop(&pda_payer_pda().0, 1_000_000_000).unwrap();
    assert!(context.layerzero_prover().lz_receive(&params).is_ok());
    assert!(proof(&context, 1).is_some());
}

#[test]
fn cancelled_claimant_passes_through() {
    let mut context = ready();
    let cancelled = Pubkey::new_from_array(CANCELLED.into());

    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, pairs(&[(1, cancelled)])))
        .unwrap();

    assert_eq!(proof(&context, 1).unwrap().claimant, cancelled);
}

/// Our own `get_proof` answers a proof `lz_receive` actually wrote (not just a
/// fabricated one): `Some` for its destination, `None` for another
/// destination or an undelivered intent.
#[test]
fn delivered_proof_is_returned_by_get_proof() {
    let mut context = ready();
    let claimant = Pubkey::new_unique();
    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, pairs(&[(1, claimant)])))
        .unwrap();

    let mut prover = context.layerzero_prover();

    assert_eq!(
        prover.get_proof([1; 32].into(), BASE_CHAIN_ID),
        Some(Proof::new(BASE_CHAIN_ID, claimant))
    );
    assert_eq!(prover.get_proof([1; 32].into(), OP_CHAIN_ID), None);
    assert_eq!(prover.get_proof([2; 32].into(), BASE_CHAIN_ID), None);
}

/// Inbound delivery → Portal `withdraw` (query tail `[proof]`, proof left in
/// place) → Portal `close_proof` (cleanup tail `[proof, pda_payer]`): the
/// reserve that paid the proof rent in `lz_receive` gets it back.
#[test]
fn delivered_proof_withdraws_and_cleanup_refunds_rent_to_pda_payer() {
    let mut context = ready();
    let (reward, route_hash, hash) = context
        .layerzero_prover()
        .funded_native_intent(BASE_CHAIN_ID, layerzero_prover::ID);
    let claimant = Pubkey::new_unique();
    let data = ProofData::new(
        BASE_CHAIN_ID,
        vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())],
    );
    let pda_payer_before = context.balance(&pda_payer_pda().0);
    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, data))
        .unwrap();
    let proof_address = Proof::pda(&hash, &layerzero_prover::ID).0;

    let result = context.portal().withdraw_intent(
        BASE_CHAIN_ID,
        reward.clone(),
        vault_pda(&hash).0,
        route_hash,
        claimant,
        proof_address,
        WithdrawnMarker::pda(&hash).0,
        Vec::<AccountMeta>::new(),
        Vec::<AccountMeta>::new(),
    );

    assert!(result.is_ok());
    assert_eq!(context.balance(&claimant), reward.native_amount);
    assert!(context.get_account(&proof_address).is_some());

    context
        .portal()
        .close_proof(BASE_CHAIN_ID, route_hash, reward, cleanup_tail(&hash))
        .unwrap();

    assert!(context.get_account(&proof_address).is_none());
    assert_eq!(context.balance(&pda_payer_pda().0), pda_payer_before);
}

/// As an aggregator member: `get_proof` returns our delivered proof through
/// the member framing, republished under the aggregator's ID, and an intent
/// naming the aggregator withdraws and cleans up through it.
#[test]
fn delivered_proof_answers_through_the_aggregator() {
    let mut context = ready();
    let aggregator_authority = Keypair::new();
    context
        .aggregator_prover()
        .install(aggregator_authority.pubkey());
    context
        .aggregator_prover()
        .init(&aggregator_authority, vec![layerzero_prover::ID])
        .unwrap();
    let (reward, route_hash, hash) = context
        .layerzero_prover()
        .funded_native_intent(BASE_CHAIN_ID, aggregator_prover::ID);
    let claimant = Pubkey::new_unique();
    let data = ProofData::new(
        BASE_CHAIN_ID,
        vec![IntentHashClaimant::new(hash, claimant.to_bytes().into())],
    );
    let pda_payer_before = context.balance(&pda_payer_pda().0);
    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, data))
        .unwrap();
    let proof_address = Proof::pda(&hash, &layerzero_prover::ID).0;

    let result = context
        .aggregator_prover()
        .get_proof(
            GetProofArgs::new(hash, BASE_CHAIN_ID, vec![]),
            &[layerzero_prover::ID],
        )
        .unwrap();
    assert_eq!(result.return_data.program_id, aggregator_prover::ID);
    assert_eq!(
        Option::<Proof>::try_from_slice(&result.return_data.data).unwrap(),
        Some(Proof::new(BASE_CHAIN_ID, claimant))
    );
    // Another destination is not evidence for this intent.
    let result = context
        .aggregator_prover()
        .get_proof(
            GetProofArgs::new(hash, OP_CHAIN_ID, vec![]),
            &[layerzero_prover::ID],
        )
        .unwrap();
    assert_eq!(
        Option::<Proof>::try_from_slice(&result.return_data.data).unwrap(),
        None
    );

    let result = context.portal().withdraw_intent(
        BASE_CHAIN_ID,
        reward.clone(),
        vault_pda(&hash).0,
        route_hash,
        claimant,
        aggregator_prover::state::Config::pda().0,
        WithdrawnMarker::pda(&hash).0,
        Vec::<AccountMeta>::new(),
        common::aggregator_query(&hash, &[layerzero_prover::ID]),
    );
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(context.balance(&claimant), reward.native_amount);
    assert!(context.get_account(&proof_address).is_some());

    let cleanup = context
        .aggregator_prover()
        .cleanup_accounts(&hash, &[layerzero_prover::ID]);
    context
        .portal()
        .close_proof(BASE_CHAIN_ID, route_hash, reward, cleanup)
        .unwrap();

    assert!(context.get_account(&proof_address).is_none());
    assert_eq!(context.balance(&pda_payer_pda().0), pda_payer_before);
}

/// A delivered cancellation refunds immediately and leaves the proof; cleanup
/// is a separate Portal `close_proof` that waits for the reward deadline and
/// returns the rent to `pda_payer`.
#[test]
fn proven_cancellation_refunds_then_cleans_up_after_deadline() {
    let mut context = ready();
    let (reward, route_hash, hash) = context
        .layerzero_prover()
        .funded_native_intent(BASE_CHAIN_ID, layerzero_prover::ID);
    let data = ProofData::new(
        BASE_CHAIN_ID,
        vec![IntentHashClaimant::new(hash, CANCELLED)],
    );
    let pda_payer_before = context.balance(&pda_payer_pda().0);
    context
        .layerzero_prover()
        .deliver(&receive_params(&peers()[0], 1, data))
        .unwrap();
    let proof_address = Proof::pda(&hash, &layerzero_prover::ID).0;
    assert!(context.now() < reward.deadline);

    let result = context.portal().refund_cancelled_intent(
        BASE_CHAIN_ID,
        reward.clone(),
        vault_pda(&hash).0,
        route_hash,
        proof_address,
        WithdrawnMarker::pda(&hash).0,
        reward.creator,
        Vec::<AccountMeta>::new(),
        Vec::<AccountMeta>::new(),
    );

    assert!(result.is_ok(), "{result:?}");
    assert_eq!(context.balance(&reward.creator), reward.native_amount);
    assert!(context.get_account(&proof_address).is_some());

    let cleanup = cleanup_tail(&hash);
    assert!(context
        .portal()
        .close_proof(BASE_CHAIN_ID, route_hash, reward.clone(), cleanup.clone())
        .is_err_and(common::is_error(PortalError::RewardNotExpired)));
    assert!(context.get_account(&proof_address).is_some());

    context.warp_to_timestamp(reward.deadline.try_into().unwrap());
    context
        .portal()
        .close_proof(BASE_CHAIN_ID, route_hash, reward, cleanup)
        .unwrap();

    assert!(context.get_account(&proof_address).is_none());
    assert_eq!(context.balance(&pda_payer_pda().0), pda_payer_before);
}
