// programs/eco-routes/src/instructions/fulfill_intent.rs
#![allow(clippy::too_many_arguments)]

use anchor_lang::{
    prelude::*,
    solana_program::{
        hash::hashv,
        instruction::{AccountMeta, Instruction},
        keccak,
        program::{invoke, invoke_signed},
        system_program,
    },
};
use borsh::{BorshDeserialize, BorshSerialize};

use crate::{
    error::EcoRoutesError,
    hyperlane,
    instructions::{dispatch_authority_key, HandleFulfilledAckArgs, InboxPayload},
    state::{IntentMarker, MAX_CALLS},
};

#[repr(u8)]
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum MetaRole {
    Solver = 1,
    SolverPda = 2,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub enum PdaSeed {
    Static(Vec<u8>),
    Solver,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct DynamicMeta {
    pub index: u8,
    pub role: MetaRole,
    pub seeds: Vec<PdaSeed>,
    pub is_signer: bool,
    pub is_writable: bool,
    pub program_id: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SvmCallData {
    pub instruction_data: Vec<u8>,
    pub meta_count: u8,
    pub dynamic_metas: Vec<DynamicMeta>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct FulfillCall {
    pub destination: [u8; 32],
    pub calldata: SvmCallData,
    pub proof: Vec<[u8; 32]>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct FulfillIntentArgs {
    pub intent_hash: [u8; 32],
    pub calls: Vec<FulfillCall>,
    pub prover: [u8; 32],
}

#[derive(Accounts)]
#[instruction(args: FulfillIntentArgs)]
pub struct FulfillIntent<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(mut)]
    pub solver: Signer<'info>,

    #[account(
        mut,
        seeds = [b"intent_marker", args.intent_hash.as_ref()],
        bump = intent_marker.bump,
    )]
    pub intent_marker: Account<'info, IntentMarker>,

    /// CHECK: Address is enforced
    #[account(
        seeds = [b"hyperlane", b"-", b"dispatch_authority"],
        bump,
        address = dispatch_authority_key().0 @ EcoRoutesError::InvalidDispatchAuthority
    )]
    pub dispatch_authority: UncheckedAccount<'info>,

    /// CHECK: Address is enforced
    #[account(address = hyperlane::MAILBOX_ID @ EcoRoutesError::NotMailbox)]
    pub mailbox_program: UncheckedAccount<'info>,

    /// CHECK: Checked in CPI
    #[account(mut)]
    pub outbox_pda: UncheckedAccount<'info>,

    /// CHECK: Checked in CPI
    pub spl_noop_program: UncheckedAccount<'info>,

    #[account(mut)]
    pub unique_message: Signer<'info>,

    /// CHECK: Checked in CPI
    #[account(mut)]
    pub dispatched_message_pda: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

fn seed_bytes<'a>(seed: &'a PdaSeed, solver: &'a Pubkey) -> &'a [u8] {
    match seed {
        PdaSeed::Static(v) => v.as_slice(),
        PdaSeed::Solver => solver.as_ref(),
    }
}

fn derive_solver_pda(desc: &DynamicMeta, solver: &Pubkey) -> Pubkey {
    let seeds: Vec<&[u8]> = desc.seeds.iter().map(|s| seed_bytes(s, solver)).collect();
    Pubkey::find_program_address(&seeds, &desc.program_id).0
}

fn assert_dynamic_meta(desc: &DynamicMeta, ai: &AccountInfo, solver: &Pubkey) -> Result<()> {
    let expected_pubkey = match desc.role {
        MetaRole::Solver => *solver,
        MetaRole::SolverPda => derive_solver_pda(desc, solver),
    };
    require!(
        ai.key == &expected_pubkey,
        EcoRoutesError::InvalidFulfillCalls
    );
    require!(
        ai.is_signer == desc.is_signer,
        EcoRoutesError::BadSignerFlag
    );
    require!(
        ai.is_writable == desc.is_writable,
        EcoRoutesError::BadWritableFlag
    );
    Ok(())
}

fn canonical_dynamic(descs: &[DynamicMeta]) -> Vec<u8> {
    let mut out = Vec::with_capacity(descs.len() * 40);
    for d in descs {
        out.push(d.index);
        out.push(d.role as u8);
        out.push(d.is_signer as u8);
        out.push(d.is_writable as u8);
        out.extend(d.program_id.as_ref());
        out.push(d.seeds.len() as u8);
        for s in &d.seeds {
            match s {
                PdaSeed::Static(v) => {
                    out.push(0);
                    out.push(v.len() as u8);
                    out.extend(v);
                }
                PdaSeed::Solver => {
                    out.push(1);
                    out.extend([0u8; 32]); // solver zeroed
                }
            }
        }
    }
    out
}

fn canonical_leaf_hash(
    dest: &[u8; 32],
    metas: &[AccountMeta],
    ix_data: &[u8],
    dyn_descs: &[DynamicMeta],
) -> [u8; 32] {
    let mut buf = Vec::with_capacity(64 + metas.len() * 34 + ix_data.len() + dyn_descs.len() * 40);
    buf.extend(dest);
    for m in metas {
        buf.extend(m.pubkey.as_ref());
        buf.push(m.is_signer as u8);
        buf.push(m.is_writable as u8);
    }
    buf.extend(ix_data);
    buf.extend(canonical_dynamic(dyn_descs));
    hashv(&[&buf]).to_bytes()
}

fn verify_merkle(leaf: &[u8; 32], proof: &[[u8; 32]], root: &[u8; 32]) -> bool {
    let mut cur = *leaf;
    for node in proof {
        cur = if cur <= *node {
            keccak::hashv(&[&cur, node]).0
        } else {
            keccak::hashv(&[node, &cur]).0
        };
    }
    &cur == root
}

pub fn fulfill_intent(ctx: Context<FulfillIntent>, args: FulfillIntentArgs) -> Result<()> {
    let marker = &mut ctx.accounts.intent_marker;
    let solver_key = ctx.accounts.solver.key();

    require!(!marker.fulfilled, EcoRoutesError::AlreadyFulfilled);
    require!(
        Clock::get()?.unix_timestamp <= marker.deadline,
        EcoRoutesError::DeadlinePassed
    );
    require!(args.calls.len() <= MAX_CALLS, EcoRoutesError::TooManyCalls);

    let mut ai_cursor = 0usize;
    let rem = &ctx.remaining_accounts;

    for call in &args.calls {
        let cd = &call.calldata;
        let meta_total = cd.meta_count as usize;
        require!(meta_total > 0, EcoRoutesError::InvalidFulfillCalls);

        require!(
            ai_cursor + meta_total <= rem.len(),
            EcoRoutesError::InvalidFulfillCalls
        );
        let slice = &rem[ai_cursor..ai_cursor + meta_total];
        ai_cursor += meta_total;

        let mut cpi_metas: Vec<AccountMeta> = vec![AccountMeta::default(); meta_total];
        let dyn_by_index: std::collections::HashMap<u8, &DynamicMeta> =
            cd.dynamic_metas.iter().map(|d| (d.index, d)).collect();

        for (idx, ai) in slice.iter().enumerate() {
            let idx_u8 = idx as u8;
            if let Some(desc) = dyn_by_index.get(&idx_u8) {
                // dynamic
                assert_dynamic_meta(desc, ai, &solver_key)?;
                cpi_metas[idx] = AccountMeta {
                    pubkey: *ai.key,
                    is_signer: desc.is_signer,
                    is_writable: desc.is_writable,
                };
            } else {
                // static
                cpi_metas[idx] = AccountMeta {
                    pubkey: *ai.key,
                    is_signer: ai.is_signer,
                    is_writable: ai.is_writable,
                };
            }
        }

        let mut canonical = cpi_metas.clone();
        for desc in &cd.dynamic_metas {
            let i = desc.index as usize;
            canonical[i].pubkey = Pubkey::default();
        }

        let leaf = canonical_leaf_hash(
            &call.destination,
            &canonical,
            &cd.instruction_data,
            &cd.dynamic_metas,
        );
        require!(
            verify_merkle(&leaf, &call.proof, &marker.calls_root),
            EcoRoutesError::InvalidFulfillCalls
        );

        let ix = Instruction {
            program_id: Pubkey::new_from_array(call.destination),
            accounts: cpi_metas,
            data: cd.instruction_data.clone(),
        };
        invoke(&ix, slice)?;
    }

    marker.fulfilled = true;

    let ack = InboxPayload::FulfilledAck(HandleFulfilledAckArgs {
        intent_hash: args.intent_hash,
        solver: solver_key,
    })
    .try_to_vec()
    .map_err(|_| error!(EcoRoutesError::InvalidFulfillCalls))?;

    #[derive(AnchorSerialize, AnchorDeserialize)]
    struct OutboxDispatch {
        sender: Pubkey,
        destination_domain: u32,
        recipient: [u8; 32],
        message_body: Vec<u8>,
    }
    const OUTBOX_DISPATCH_VARIANT: u8 = 4;

    let outbox_dispatch = OutboxDispatch {
        sender: ctx.accounts.dispatch_authority.key(),
        destination_domain: marker.source_domain_id,
        recipient: args.prover,
        message_body: ack,
    };
    let mut ix_data = vec![OUTBOX_DISPATCH_VARIANT];
    ix_data.extend(outbox_dispatch.try_to_vec()?);

    let metas = vec![
        AccountMeta::new(ctx.accounts.outbox_pda.key(), false),
        AccountMeta::new_readonly(ctx.accounts.dispatch_authority.key(), true),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new_readonly(ctx.accounts.spl_noop_program.key(), false),
        AccountMeta::new_readonly(ctx.accounts.payer.key(), true),
        AccountMeta::new_readonly(ctx.accounts.unique_message.key(), true),
        AccountMeta::new(ctx.accounts.dispatched_message_pda.key(), false),
    ];

    invoke_signed(
        &Instruction {
            program_id: ctx.accounts.mailbox_program.key(),
            accounts: metas.clone(),
            data: ix_data,
        },
        &[
            ctx.accounts.outbox_pda.to_account_info(),
            ctx.accounts.dispatch_authority.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            ctx.accounts.spl_noop_program.to_account_info(),
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.unique_message.to_account_info(),
            ctx.accounts.dispatched_message_pda.to_account_info(),
        ],
        &[&[
            b"hyperlane",
            b"-",
            b"dispatch_authority",
            &[ctx.bumps.dispatch_authority],
        ]],
    )?;

    Ok(())
}
