use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::InstructionData;
use anchor_spl::associated_token::{self, get_associated_token_address_with_program_id};
use anchor_spl::{token, token_2022};
use eco_svm_std::Bytes32;
use portal::types::{intent_hash, TokenTransferAccounts};
use tiny_keccak::{Hasher, Keccak};

use crate::events::IntentChained;
use crate::instructions::ChainerError;
use crate::state::{escrow_authority_pda, vault_pda, withdrawn_marker_pda, ESCROW_SEED};
use crate::types::{scale_amount, AmountContext, Order};

/// Args for [`chain_intent`].
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ChainArgs {
    /// The committed intent2 template, minus the amount. Hashed to derive the
    /// escrow authority, so presenting a different order simply fails to derive
    /// the account that holds the money.
    pub order: Order,
    /// Whether to also CPI `portal::publish` so intent2 appears in portal's own
    /// `IntentPublished` stream.
    ///
    /// Only ever *strengthens* [`Order::require_publish`]: the effective decision
    /// is `publish || order.require_publish`, so a caller can add discoverability
    /// but never remove it. See that field for why it is committed.
    ///
    /// Publication does not control the local settlement checks: those always
    /// run. A published route is carried in Portal's full IntentPublished event.
    pub publish: bool,
}

/// Accounts for [`chain_intent`].
///
/// Every address is validated in the handler against the order and the resolved
/// intent hash, so nothing here is caller-chosen in a way that matters. In
/// particular `vault` and `vault_ata` cannot be pinned by an Anchor constraint:
/// they depend on the intent hash, which depends on the amount this instruction
/// has not measured yet.
#[derive(Accounts)]
pub struct Chain<'info> {
    /// Pays the vault ATA's rent when it does not exist yet. A plain signer
    /// deliberately, never a PDA: the associated-token program funds a new account
    /// with a system transfer from `payer`, and the system program refuses a
    /// transfer whose source carries data.
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: address is validated against the order commitment
    pub escrow_authority: UncheckedAccount<'info>,
    /// CHECK: address is validated as the escrow authority's derived ATA
    #[account(mut)]
    pub escrow_ata: UncheckedAccount<'info>,
    /// CHECK: address is validated to equal `order.base_mint`
    pub base_mint: UncheckedAccount<'info>,
    /// CHECK: address is validated as `vault_pda(intent_hash)`
    #[account(mut)]
    pub vault: UncheckedAccount<'info>,
    /// CHECK: address is validated as the vault's derived ATA
    #[account(mut)]
    pub vault_ata: UncheckedAccount<'info>,
    /// CHECK: address is validated as `WithdrawnMarker::pda(intent_hash)`
    pub withdrawn_marker: UncheckedAccount<'info>,
    /// CHECK: address is validated to equal the order's committed `portal`
    ///
    /// Deliberately not pinned to a linked-in `portal::ID`. One chainer serves any
    /// number of portal deployments; which one an order uses is the order author's
    /// choice, committed and therefore covered by intent1's hash. `executable` is
    /// kept so a non-program address fails here rather than deep inside the CPI.
    #[account(executable)]
    pub portal_program: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub associated_token_program: Program<'info, associated_token::AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Measure the escrow's balance of one mint and fund a follow-on intent with it.
///
/// Validate the order, amounts and derived accounts before moving value. Transfer
/// or publication failures roll back the transaction, leaving the escrow intact.
///
/// Unlike EVM's atomic chain, this cannot unwind intent1's earlier fulfillment.
/// Retries must preserve the committed order; correcting a stale account list
/// does not authorize changing the order or its deadline.
pub fn chain_intent<'info>(ctx: Context<'info, Chain<'info>>, args: ChainArgs) -> Result<()> {
    chain_order(ctx, args, None)
}

/// One validation/value-movement path for both transports. A staged invocation
/// also checks the immutable buffer's declared commitment, hashing only once.
pub(crate) fn chain_order<'info>(
    ctx: Context<'info, Chain<'info>>,
    args: ChainArgs,
    buffered_commitment: Option<Bytes32>,
) -> Result<()> {
    let ChainArgs { order, publish } = args;
    // The caller may strengthen the author's choice, never weaken it.
    let publish = publish || order.require_publish;

    validate_order(&order)?;
    let order_commitment = order.hash();
    if let Some(expected) = buffered_commitment {
        require!(
            order_commitment == expected,
            ChainerError::OrderCommitmentMismatch
        );
    }
    let escrow = validate_escrow(&ctx, &order, &order_commitment)?;

    let amount_in = measure(&ctx, &order)?;
    let amount_out = scale_amount(amount_in as u128, order.scale)?;

    let route = order.template.render(AmountContext {
        input: amount_in,
        output: amount_out,
    })?;
    let route_hash = keccak(&route);
    let mut reward = order.reward.clone();
    reward.tokens[0].amount = amount_in;

    let intent_hash = intent_hash(order.destination, &route_hash, &reward.hash());
    validate_destination(&ctx, &order, &intent_hash)?;

    push(&ctx, &order_commitment, escrow, amount_in)?;

    if publish {
        publish_intent(&ctx, order.destination, route, reward)?;
    }

    emit!(IntentChained::new(
        intent_hash,
        order_commitment,
        ctx.accounts.vault.key(),
        order.base_mint,
        amount_in,
        amount_out,
        order.destination,
        route_hash,
        publish,
    ));

    #[cfg(all(feature = "resource-metrics", target_os = "solana"))]
    crate::log_heap_usage("complete");

    Ok(())
}

/// Rejects a malformed order before anything is measured or moved.
pub(crate) fn validate_order(order: &Order) -> Result<()> {
    order.validate_template()?;

    require!(order.scale > 0, ChainerError::InvalidScale);

    // Exactly one leg, in the measured mint, authored at zero.
    //
    // This program transfers only the measured mint. It cannot guarantee funding
    // for another reward leg, even if a caller separately prefunds that vault.
    // Portal's `refund` refuses while a `Proof` exists, while `withdraw` pays only
    // what the vault actually holds.
    require!(
        order.reward.tokens.len() == 1,
        ChainerError::InvalidRewardLegCount
    );
    require!(
        order.reward.tokens[0].token == order.base_mint,
        ChainerError::RewardTokenMismatch
    );
    require!(
        order.reward.tokens[0].amount == 0,
        ChainerError::RewardAmountMustBeZero
    );

    // A native reward would need this program to move lamports out of a PDA that
    // also holds tokens, and portal's `fund` native leg drains a short funder's
    // entire balance. None of it is needed for a token-in/token-out chain.
    require!(
        order.reward.native_amount == 0,
        ChainerError::NativeRewardNotSupported
    );

    // Do not gate this transfer on time: unlike the atomic EVM chain, intent1
    // has already settled before this instruction runs. Rejecting a late order
    // would strand its escrow. Preserve the committed deadline and let Portal
    // decide whether the funded reward is refundable or claimable. Deadline
    // headroom belongs in the builder's admission checks, before intent1 is funded.

    Ok(())
}

/// Escrow custody, resolved from the order commitment.
struct Escrow {
    bump: u8,
}

/// Binds the accounts holding the money to the order that was presented.
///
/// This is the whole authorization story: the escrow authority is
/// `escrow_authority_pda(keccak(borsh(order)))`, so an order the caller invented
/// derives an authority that holds nothing, and the order intent1 committed to
/// derives the one that does. No signer check is needed or useful — see
/// [`crate::state::escrow_authority_pda`].
fn validate_escrow(
    ctx: &Context<Chain>,
    order: &Order,
    order_commitment: &Bytes32,
) -> Result<Escrow> {
    // The portal is the order's, never the caller's. Everything downstream — the
    // vault the escrow sweeps into, the marker consulted for an already-settled
    // hash, the program `publish` dispatches to — derives from it, so a
    // caller-chosen portal would let an attacker redirect the sweep into a PDA of
    // a program they wrote while the commitment still appeared to authorise it.
    require!(
        ctx.accounts.portal_program.key() == order.portal,
        ChainerError::InvalidPortalProgram
    );

    let (expected_authority, bump) = escrow_authority_pda(order_commitment);
    require!(
        ctx.accounts.escrow_authority.key() == expected_authority,
        ChainerError::InvalidEscrowAuthority
    );
    require!(
        ctx.accounts.base_mint.key() == order.base_mint,
        ChainerError::InvalidMint
    );

    let expected_ata = get_associated_token_address_with_program_id(
        &expected_authority,
        &order.base_mint,
        ctx.accounts.base_mint.owner,
    );
    require!(
        ctx.accounts.escrow_ata.key() == expected_ata,
        ChainerError::InvalidEscrowAta
    );

    Ok(Escrow { bump })
}

/// The single runtime measurement in the whole flow.
fn measure<'info>(ctx: &Context<'info, Chain<'info>>, order: &Order) -> Result<u64> {
    let escrow_ata = transfer_accounts(ctx)?;
    let amount_in = escrow_ata.from_data()?.amount;

    require!(amount_in > 0, ChainerError::ZeroAmount);
    require!(
        amount_in >= order.min_amount_in,
        ChainerError::AmountBelowFloor
    );

    Ok(amount_in)
}

/// Checks the caller-supplied destination accounts against the resolved hash.
///
/// The amount is measured on-chain, but the accounts it implies must be named in
/// the transaction before it runs, so the caller derives them off-chain from the
/// balance they observed and this proves they got it right. A donation into the
/// escrow between their read and this instruction moves the hash and fails here —
/// loudly, with the escrow untouched, rather than funding the wrong vault.
fn validate_destination(ctx: &Context<Chain>, order: &Order, intent_hash: &Bytes32) -> Result<()> {
    require!(
        ctx.accounts.vault.key() == vault_pda(&order.portal, intent_hash).0,
        ChainerError::InvalidVault
    );

    let expected_vault_ata = get_associated_token_address_with_program_id(
        ctx.accounts.vault.key,
        &order.base_mint,
        ctx.accounts.base_mint.owner,
    );
    require!(
        ctx.accounts.vault_ata.key() == expected_vault_ata,
        ChainerError::InvalidVaultAta
    );

    // Portal decides a reward's fate from live vault balances and a one-shot
    // `WithdrawnMarker`, never from a funded flag — which is exactly why pushing
    // directly into the vault works at all. The flip side is that a push after
    // withdrawal is unrecoverable by the claimant, so refuse it.
    //
    // This corresponds to the EVM chainer's unconditional Portal status check,
    // independent of publication — but it is **narrower**, and
    // the difference is worth being precise about. `portal::refund` never creates
    // a marker; it only reads one, to permit the post-withdrawal sweep
    // (`refund.rs:82`). So a *refunded* intent leaves no marker and passes this
    // check, where on EVM `Status.Refunded` is terminal.
    //
    // A fresh child route salt is the builder's responsibility. Reusing every
    // hashed field and the measured amount funds the SAME child intent, including
    // any delayed proof for it; a different parent or order does not create a new
    // claim. Neither a vault balance nor this marker proves child-hash uniqueness.
    // Portal's refund/late-proof semantics are unchanged here.
    require!(
        ctx.accounts.withdrawn_marker.key() == withdrawn_marker_pda(&order.portal, intent_hash).0,
        ChainerError::InvalidWithdrawnMarker
    );
    require!(
        ctx.accounts.withdrawn_marker.data_is_empty(),
        ChainerError::IntentAlreadySettled
    );

    Ok(())
}

/// Creates intent2's vault ATA if needed and moves the measured balance into it.
///
/// A direct push rather than a `portal::fund` CPI, matching the EVM contract's
/// choice and for the same reason: portal's `withdraw` pays
/// `min(reward_amount, vault_ata.amount)` with no funded flag anywhere, so a
/// pushed intent is fully withdrawable by the proven claimant. `fund` would buy an
/// event and cost three hazards — it needs `funder` to be a `Signer`, its
/// ATA-creating path does a system transfer from `payer` (which forbids a
/// data-carrying PDA there), and its native leg drains a short funder outright.
fn push<'info>(
    ctx: &Context<'info, Chain<'info>>,
    order_commitment: &Bytes32,
    escrow: Escrow,
    amount_in: u64,
) -> Result<()> {
    let accounts = transfer_accounts(ctx)?;
    let token_program = accounts.token_program(
        &ctx.accounts.token_program,
        &ctx.accounts.token_2022_program,
    )?;

    if ctx.accounts.vault_ata.data_is_empty() {
        associated_token::create_idempotent(CpiContext::new(
            ctx.accounts.associated_token_program.key(),
            associated_token::Create {
                payer: ctx.accounts.payer.to_account_info(),
                associated_token: ctx.accounts.vault_ata.to_account_info(),
                authority: ctx.accounts.vault.to_account_info(),
                mint: ctx.accounts.base_mint.to_account_info(),
                system_program: ctx.accounts.system_program.to_account_info(),
                token_program: token_program.to_account_info(),
            },
        ))?;
    }

    // Prefunding is allowed at any balance. Child-hash uniqueness belongs to the
    // builder; this snapshot measures only what the current transfer delivers.
    let balance_before = accounts.to_data()?.amount;

    // The commitment is threaded in rather than recomputed: `Order::hash` keccaks
    // the whole order, route segments included, and once per call is enough.
    let signer_seeds = [ESCROW_SEED, order_commitment.as_ref(), &[escrow.bump]];

    accounts.transfer_with_signer(
        &token_program,
        &ctx.accounts.escrow_authority,
        &[&signer_seeds],
        amount_in,
    )?;

    // Existing funds must not mask an outbound transfer fee. Reject any short
    // delivery atomically, restoring both the escrow and the prefunded vault.
    // This does not attest to arbitrary Token-2022 extension semantics.
    require!(
        accounts.to_data()?.amount.checked_sub(balance_before) == Some(amount_in),
        ChainerError::PushShortfall
    );

    Ok(())
}

/// CPIs `portal::publish` so intent2 joins portal's own event stream.
///
/// Legal only because `chain` runs in its own transaction. Called as a route call
/// inside intent1's fulfillment this would be `portal → chainer → portal`, and the
/// runtime rejects it with `ReentrancyNotAllowed`: `invoke_context.rs` refuses any
/// CPI to a program already on the instruction stack unless it is calling itself.
/// That is the same rule that keeps `flash-fulfiller` a separate program.
///
/// `portal::publish` takes no accounts at all — it hashes the route and emits —
/// so the CPI carries only the program itself.
fn publish_intent(
    ctx: &Context<Chain>,
    destination: u64,
    route: Vec<u8>,
    reward: portal::types::Reward,
) -> Result<()> {
    let instruction = Instruction {
        program_id: ctx.accounts.portal_program.key(),
        accounts: vec![],
        data: portal::instruction::Publish {
            args: portal::instructions::PublishArgs {
                destination,
                route,
                reward,
            },
        }
        .data(),
    };

    invoke(
        &instruction,
        &[ctx.accounts.portal_program.to_account_info()],
    )
    .map_err(Into::into)
}

/// The `[from, to, mint]` triple portal's own token plumbing operates on, reused
/// verbatim so this program's transfer semantics — spl-token versus token-2022
/// discrimination by mint owner, decimals from the mint — cannot drift from
/// portal's.
fn transfer_accounts<'info>(
    ctx: &Context<'info, Chain<'info>>,
) -> Result<TokenTransferAccounts<'info>> {
    vec![
        &ctx.accounts.escrow_ata.to_account_info(),
        &ctx.accounts.vault_ata.to_account_info(),
        &ctx.accounts.base_mint.to_account_info(),
    ]
    .try_into()
}

fn keccak(bytes: &[u8]) -> Bytes32 {
    let mut hasher = Keccak::v256();
    let mut hash = [0u8; 32];

    hasher.update(bytes);
    hasher.finalize(&mut hash);

    hash.into()
}
