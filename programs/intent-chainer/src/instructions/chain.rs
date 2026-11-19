use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::Instruction;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::Discriminator;
use anchor_spl::associated_token::{self, get_associated_token_address_with_program_id};
use anchor_spl::{token, token_2022};
use eco_svm_std::Bytes32;
use portal::types::{intent_hash, TokenTransferAccounts};

use crate::events::IntentChained;
use crate::instructions::ChainerError;
use crate::state::{escrow_authority_pda, vault_pda, ESCROW_SEED};
use crate::types::{
    scale_amount, AmountContext, Order, TemplateProgram, TemplateProgramView,
    MAX_PUBLISHED_ROUTE_LEN,
};

/// Args for [`chain_intent`].
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ChainArgs {
    /// The committed intent2 parameters, minus the amount. Hashed with
    /// [`ChainArgs::template`] to derive the escrow authority, so presenting a
    /// different order simply fails to derive the account that holds the money.
    pub order: Order,
    /// The template preimage `order.template_hash` commits to. Carried only in the
    /// transaction that executes this order: a parent referencing this order as a
    /// child needs the two hashes, never these bytes.
    pub template: TemplateProgram,
    /// Whether to also CPI `portal::publish` so intent2 appears in portal's own
    /// `IntentPublished` stream.
    ///
    /// The caller's choice alone: the order does not commit to it, so a publish refused
    /// as too long ([`ChainerError::PublishRouteTooLong`]) is retried without one.
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
    let ChainArgs {
        order,
        template,
        publish,
    } = args;
    let hash = template.hash()?;
    let encoded_len = crate::types::borsh_len(&template)?;

    chain_order(
        ctx,
        order,
        PresentedTemplate {
            view: template.view(),
            hash,
            encoded_len,
        },
        publish,
    )
}

/// A template as `chain` is handed it, read in place, with its commitment and its
/// Borsh length already known: computed from the instruction's owned template for an
/// inline order, taken from the buffer account for a staged one.
pub(crate) struct PresentedTemplate<'a> {
    pub(crate) view: TemplateProgramView<'a>,
    pub(crate) hash: Bytes32,
    pub(crate) encoded_len: usize,
}

/// One validation/value-movement path for both transports. A staged invocation
/// also checks the immutable buffer's declared commitment, hashing only once.
pub(crate) fn chain_order<'info>(
    ctx: Context<'info, Chain<'info>>,
    order: Order,
    template: PresentedTemplate<'_>,
    publish: bool,
) -> Result<()> {
    let root_len = validate_order(&order, &template)?;
    // Portal logs a published route whole, and the runtime truncates a log past its
    // budget without an error. Refuse before anything moves, so a refused publish
    // leaves the escrow chainable without one.
    require!(
        !publish || root_len <= MAX_PUBLISHED_ROUTE_LEN,
        ChainerError::PublishRouteTooLong
    );
    let params_hash = order.params_hash()?;
    let escrow = validate_escrow(&ctx, &order, &params_hash)?;

    let amount_in = measure(&ctx, &order)?;
    let amount_out = scale_amount(amount_in as u128, order.scale)?;

    // The route is committed by hash from where its segments lie. It is rendered into
    // memory only to publish it.
    let context = AmountContext {
        input: amount_in,
        output: amount_out,
    };
    let recipients = template.view.recipients(context)?;
    let route_hash: Bytes32 = template.view.route_hash(context, &recipients)?.into();
    let mut reward = order.reward.clone();
    reward.tokens[0].amount = amount_in;

    let intent_hash = intent_hash(order.destination, &route_hash, &reward.hash());
    validate_destination(&ctx, &order, &intent_hash)?;

    push(&ctx, &order, &params_hash, escrow, amount_in)?;

    if publish {
        let route = template.view.render(context, &recipients, root_len)?;
        publish_intent(&ctx, order.destination, route, reward)?;
    }

    emit!(IntentChained::new(
        intent_hash,
        order.template_hash,
        params_hash,
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
pub(crate) fn validate_order(order: &Order, template: &PresentedTemplate<'_>) -> Result<usize> {
    order.validate_params()?;
    // The template is supplied, not committed inline, so bind it before use.
    require!(
        template.hash == order.template_hash,
        ChainerError::TemplateCommitmentMismatch
    );
    let root_len = template.view.validate(template.encoded_len)?;

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

    Ok(root_len)
}

/// Escrow custody, resolved from the order commitment.
struct Escrow {
    bump: u8,
}

/// Binds the accounts holding the money to the order that was presented.
///
/// This is the whole authorization story: the escrow authority is
/// `escrow_authority_pda(template_hash, params_hash)`, so an order the caller invented
/// derives an authority that holds nothing, and the order intent1 committed to
/// derives the one that does. No signer check is needed or useful — see
/// [`crate::state::escrow_authority_pda`].
fn validate_escrow(ctx: &Context<Chain>, order: &Order, params_hash: &Bytes32) -> Result<Escrow> {
    // The portal is the order's, never the caller's. Everything downstream — the
    // vault the escrow sweeps into, the marker consulted for an already-settled
    // hash, the program `publish` dispatches to — derives from it, so a
    // caller-chosen portal would let an attacker redirect the sweep into a PDA of
    // a program they wrote while the commitment still appeared to authorise it.
    require!(
        ctx.accounts.portal_program.key() == order.portal,
        ChainerError::InvalidPortalProgram
    );

    let (expected_authority, bump) = escrow_authority_pda(&order.template_hash, params_hash);
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

    // Deliberately NOT gated on intent2's `WithdrawnMarker`. The EVM chainer does
    // check Portal status here; do not port that check back. The difference is
    // WHERE THE BALANCE SITS WHEN THE CHECK RUNS, not the two portals' refund
    // rules — those are in fact the same shape, and assuming otherwise is what put
    // this check here originally.
    //
    // On EVM `chain()` is intent1's last route call, so a revert unwinds the swap
    // that produced the balance: nothing is ever at rest, nothing is bound to an
    // order, and refusing costs only that fulfillment. There the check is a
    // cheap fail-fast.
    //
    // Here `chain` is necessarily its own transaction, so the balance IS at rest,
    // and it is at rest in `escrow_authority_pda(template_hash, params_hash)` — bound to
    // exactly one order. This instruction is that escrow's only exit: there is no
    // sweep, no recovery instruction, and no later call can clear a marker. A
    // refusal is therefore permanent for that balance, so any check able to reject
    // a state that is otherwise fundable converts a recoverable state into an
    // unrecoverable one. That asymmetry is the whole argument.
    //
    // Nor is the marker anyone's to avoid: `withdraw` pays
    // `min(reward_amount, vault_ata.amount)` and marks even when it pays zero, so
    // an empty vault can be "withdrawn" by anyone at any time. And the push does
    // land somewhere recoverable — `portal::refund` opens with
    // `if !withdrawn_marker.data_is_empty() { return Ok(()) }` (`refund.rs:82`),
    // paying live vault balances to `reward.creator` with no deadline wait and no
    // proof check.
    //
    // A fresh child route salt remains the builder's responsibility. Reusing every
    // hashed field and the measured amount funds the SAME child intent, including
    // any delayed proof for it; neither a vault balance nor a marker proves child-
    // hash uniqueness. Portal's refund/late-proof semantics are unchanged here.

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
    order: &Order,
    params_hash: &Bytes32,
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

    // Both halves of the commitment, in the same order `escrow_authority_pda`
    // derives them. Threaded in rather than recomputed; once per call is enough.
    let signer_seeds = [
        ESCROW_SEED,
        order.template_hash.as_ref(),
        params_hash.as_ref(),
        &[escrow.bump],
    ];

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
    // Built into one buffer of exactly its length: `InstructionData::data` starts at 256
    // bytes and doubles as the route and reward are written, and this program's
    // allocator never gives the discarded buffers back.
    let args = portal::instructions::PublishArgs {
        destination,
        route,
        reward,
    };
    let mut data = Vec::with_capacity(
        portal::instruction::Publish::DISCRIMINATOR.len() + crate::types::borsh_len(&args)?,
    );
    data.extend_from_slice(portal::instruction::Publish::DISCRIMINATOR);
    args.serialize(&mut data)?;
    let instruction = Instruction {
        program_id: ctx.accounts.portal_program.key(),
        accounts: vec![],
        data,
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
