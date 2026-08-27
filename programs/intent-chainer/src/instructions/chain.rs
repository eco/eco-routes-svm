use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::InstructionData;
use anchor_spl::associated_token::{self, get_associated_token_address_with_program_id};
use anchor_spl::token_interface::TokenAccount;
use anchor_spl::{token, token_2022};
use eco_svm_std::Bytes32;
use portal::state::{vault_pda, WithdrawnMarker};
use portal::types::{intent_hash, TokenTransferAccounts};
use tiny_keccak::{Hasher, Keccak};

use crate::events::IntentChained;
use crate::instructions::{now, ChainerError};
use crate::state::{escrow_authority_pda, ESCROW_SEED};
use crate::types::{scale_amount, Order, MAX_ROUTE_LEN, MAX_SLOTS, MIN_DEADLINE_BUFFER};

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
    /// A real choice on Solana, unlike on EVM where publish is unconditional
    /// because it is the only way to learn the vault address and to reject an
    /// already-settled hash. Here the vault is a derivable PDA and the settled
    /// check reads `WithdrawnMarker` directly, so publish buys **only**
    /// discoverability — and it costs the route bytes twice over in log budget and
    /// an in-program keccak of the whole route inside portal.
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
#[instruction(args: ChainArgs)]
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
    /// CHECK: address is validated to equal `portal::ID`
    #[account(executable, address = portal::ID @ ChainerError::InvalidPortalProgram)]
    pub portal_program: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub associated_token_program: Program<'info, associated_token::AssociatedToken>,
    pub system_program: Program<'info, System>,
}

/// Measure the escrow's balance of one mint and fund a follow-on intent with it.
///
/// Ordering is load-bearing: everything that can fail — the shape, the floor, the
/// slot widths, the deadline buffer, the already-settled check, every address
/// derivation — is checked **before** any value moves, so a rejected order leaves
/// the escrow exactly as it was and the call can be retried with a corrected one.
///
/// That is weaker than the EVM contract's guarantee and the difference is worth
/// stating rather than eliding. There, `chain` runs inside intent1's fulfillment,
/// so a failure reverts **intent1 whole** — the solver's input is untouched and
/// the swap never happened. Here intent1 has already been fulfilled and settled in
/// an earlier transaction; nothing can unwind it. What survives is only that the
/// measured balance stays in the escrow, recoverable by re-running `chain` with a
/// corrected order. Preserving the escrow is the whole of the guarantee.
pub fn chain_intent<'info>(ctx: Context<'info, Chain<'info>>, args: ChainArgs) -> Result<()> {
    let ChainArgs { order, publish } = args;
    // The caller may strengthen the author's choice, never weaken it.
    let publish = publish || order.require_publish;

    validate_order(&order)?;
    let order_commitment = order.hash();
    let escrow = validate_escrow(&ctx, &order, &order_commitment)?;

    let amount_in = measure(&ctx, &order)?;
    let amount_out = scale_amount(amount_in, order.scale)?;

    let route = order.build_route(amount_out)?;
    let route_hash = keccak(&route);
    let mut reward = order.reward.clone();
    reward.tokens[0].amount = amount_in;

    let intent_hash = intent_hash(order.destination, &route_hash, &reward.hash());
    validate_destination(&ctx, &order, &intent_hash)?;
    validate_vault_is_unfunded(&ctx, amount_in)?;

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

    Ok(())
}

/// Rejects a malformed order before anything is measured or moved.
fn validate_order(order: &Order) -> Result<()> {
    // Shape first, and specifically *before* `Order::hash` streams the whole order
    // through keccak in-program.
    //
    // `Order::build_route` re-checks all three. That is deliberate rather than
    // redundant: it is a public constructor the SDK and tests call directly, so it
    // cannot assume a caller ran this first. The duplication is safe because both
    // sites read the same constants — if they are ever made to differ, the one
    // here is the gate and the one there is the invariant. That hash costs compute proportional to the
    // order's size, so an over-length order that is going to be rejected anyway
    // must be rejected while it is still cheap — otherwise the caller pays the
    // full hash to be told the route was too long, and at the default compute
    // limit runs out before ever hearing it.
    require!(order.slots.len() <= MAX_SLOTS, ChainerError::TooManySlots);
    require!(
        order.segments.len() == order.slots.len() + 1,
        ChainerError::SegmentCountMismatch
    );
    require!(
        order.route_len()? <= MAX_ROUTE_LEN,
        ChainerError::RouteTooLong
    );

    require!(order.scale > 0, ChainerError::InvalidScale);

    // Exactly one leg, in the measured mint, authored at zero.
    //
    // A leg in any other mint could never be funded from here — the vault address
    // is unknowable until the amount is measured, so nothing can pre-fund it — and
    // once such an intent is proven its vault is stuck: portal's `refund` refuses
    // for as long as a `Proof` exists, while `withdraw` pays only what the vault
    // actually holds.
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

    require!(
        order.reward.deadline >= now()?.saturating_add(MIN_DEADLINE_BUFFER),
        ChainerError::DeadlineTooSoon
    );

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
        ctx.accounts.vault.key() == vault_pda(intent_hash).0,
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
    // This stands in for the EVM `publish`'s already-settled rejection, which
    // portal's stateless `publish` cannot provide — but it is **narrower**, and
    // the difference is worth being precise about. `portal::refund` never creates
    // a marker; it only reads one, to permit the post-withdrawal sweep
    // (`refund.rs:82`). So a *refunded* intent leaves no marker and passes this
    // check, where on EVM `Status.Refunded` is terminal.
    //
    // Not believed exploitable: two orders resolving to the same intent hash carry
    // an identical reward by construction, so a re-funded intent still pays its own
    // claimant or refunds to the same creator. Closing it properly would mean
    // portal recording refunds, which is a portal decision, not one this program
    // can make.
    require!(
        ctx.accounts.withdrawn_marker.key() == WithdrawnMarker::pda(intent_hash).0,
        ChainerError::InvalidWithdrawnMarker
    );
    require!(
        ctx.accounts.withdrawn_marker.data_is_empty(),
        ChainerError::IntentAlreadySettled
    );

    Ok(())
}

/// Refuses a salt collision before it becomes a silent merge.
///
/// Two orders sharing a salt that resolve to the same measured amount produce the
/// same intent hash, so the second push would top up the first intent's vault
/// rather than create a second intent — funding a delivery that already has a
/// claimant, with no signal that anything went wrong. The EVM contract cannot
/// reach this state: its `publish` rejects an already-settled hash and unwinds
/// intent1 whole. Portal's `publish` is stateless and offers no such check, so it
/// is made here.
///
/// Nothing legitimate pre-funds intent2's vault: the address depends on the
/// measured amount, so it is unknowable before this instruction runs. A balance
/// already at or above what is being pushed therefore means the hash is not fresh.
fn validate_vault_is_unfunded(ctx: &Context<Chain>, amount_in: u64) -> Result<()> {
    let vault_ata = &ctx.accounts.vault_ata;
    if vault_ata.data_is_empty() {
        return Ok(());
    }

    let funded = TokenAccount::try_deserialize(&mut &vault_ata.try_borrow_data()?[..])
        .map(|account| account.amount)
        .unwrap_or_default();

    require!(funded < amount_in, ChainerError::VaultAlreadyFunded);

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

    // The commitment is threaded in rather than recomputed: `Order::hash` keccaks
    // the whole order, route segments included, and once per call is enough.
    let signer_seeds = [ESCROW_SEED, order_commitment.as_ref(), &[escrow.bump]];

    accounts.transfer_with_signer(
        &token_program,
        &ctx.accounts.escrow_authority,
        &[&signer_seeds],
        amount_in,
    )?;

    // Portal reads the vault's live balance to decide what a claimant is owed, so a
    // mint that delivers less than it was sent (a token-2022 transfer fee) would
    // leave intent2 quietly payable only in part. Reject it while the escrow is
    // still whole. This is the EVM `PushShortfall` check.
    require!(
        accounts.to_data()?.amount >= amount_in,
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
