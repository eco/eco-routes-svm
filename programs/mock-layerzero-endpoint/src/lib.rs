//! Test-only stand-in for LayerZero's EndpointV2 (localnet only; excluded from
//! devnet/mainnet builds). Instruction names, params and account layouts match
//! LayerZero-v2@9c741e7f so the discriminators, seeds and account orders
//! layerzero-prover mirrors resolve here unchanged. Library/worker accounts are
//! accepted and ignored; `set_config` and `send` emit events instead of
//! touching a ULN so tests can assert exactly what was configured or sent.
//! `mock_init` / `mock_verify` stand in for LayerZero's admin setup and DVN
//! verification.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::solana_program::system_instruction;
use tiny_keccak::{Hasher, Keccak};

declare_id!("76y77prsiCMvXMjuoZ5VRrhG5qYBrUMYTE5WgHqgjEn6");

pub const MOCK_NATIVE_FEE: u64 = 1_000_000;
const SEND_DISCRIMINATOR: [u8; 8] = [102, 251, 20, 187, 65, 75, 12, 69];

const ENDPOINT_SEED: &[u8] = b"Endpoint";
const OAPP_SEED: &[u8] = b"OApp";
const NONCE_SEED: &[u8] = b"Nonce";
const PENDING_NONCE_SEED: &[u8] = b"PendingNonce";
const PAYLOAD_HASH_SEED: &[u8] = b"PayloadHash";
const SEND_LIBRARY_CONFIG_SEED: &[u8] = b"SendLibraryConfig";
const RECEIVE_LIBRARY_CONFIG_SEED: &[u8] = b"ReceiveLibraryConfig";
const MESSAGE_LIB_SEED: &[u8] = b"MessageLib";

#[program]
pub mod mock_layerzero_endpoint {
    use super::*;

    pub fn mock_init(ctx: Context<MockInit>, eid: u32) -> Result<()> {
        ctx.accounts.endpoint.set_inner(EndpointSettings {
            eid,
            bump: ctx.bumps.endpoint,
            admin: ctx.accounts.payer.key(),
            lz_token_mint: None,
        });
        Ok(())
    }

    pub fn mock_verify(ctx: Context<MockVerify>, params: MockVerifyParams) -> Result<()> {
        ctx.accounts.payload_hash.set_inner(PayloadHash {
            hash: params.payload_hash,
            bump: ctx.bumps.payload_hash,
        });
        let nonce = &mut ctx.accounts.nonce;
        nonce.inbound_nonce = nonce.inbound_nonce.max(params.nonce);
        Ok(())
    }

    pub fn register_oapp(ctx: Context<RegisterOApp>, params: RegisterOAppParams) -> Result<()> {
        ctx.accounts.oapp_registry.set_inner(OAppRegistry {
            delegate: params.delegate,
            bump: ctx.bumps.oapp_registry,
        });
        Ok(())
    }

    pub fn init_nonce(ctx: Context<InitNonce>, _params: InitNonceParams) -> Result<()> {
        ctx.accounts.nonce.set_inner(Nonce {
            bump: ctx.bumps.nonce,
            outbound_nonce: 0,
            inbound_nonce: 0,
        });
        ctx.accounts
            .pending_inbound_nonce
            .set_inner(PendingInboundNonce {
                nonces: vec![],
                bump: ctx.bumps.pending_inbound_nonce,
            });
        Ok(())
    }

    pub fn init_send_library(
        ctx: Context<InitSendLibrary>,
        _params: InitSendLibraryParams,
    ) -> Result<()> {
        ctx.accounts
            .send_library_config
            .set_inner(SendLibraryConfig {
                message_lib: Pubkey::default(),
                bump: ctx.bumps.send_library_config,
            });
        Ok(())
    }

    pub fn init_receive_library(
        ctx: Context<InitReceiveLibrary>,
        _params: InitReceiveLibraryParams,
    ) -> Result<()> {
        ctx.accounts
            .receive_library_config
            .set_inner(ReceiveLibraryConfig {
                message_lib: Pubkey::default(),
                timeout: None,
                bump: ctx.bumps.receive_library_config,
            });
        Ok(())
    }

    pub fn set_send_library(
        ctx: Context<SetSendLibrary>,
        params: SetSendLibraryParams,
    ) -> Result<()> {
        ctx.accounts.send_library_config.message_lib = params.new_lib;
        Ok(())
    }

    pub fn set_receive_library(
        ctx: Context<SetReceiveLibrary>,
        params: SetReceiveLibraryParams,
    ) -> Result<()> {
        ctx.accounts.receive_library_config.message_lib = params.new_lib;
        Ok(())
    }

    pub fn init_config<'info>(
        ctx: Context<'info, InitConfig<'info>>,
        params: InitConfigParams,
    ) -> Result<()> {
        let payer = ctx
            .remaining_accounts
            .first()
            .ok_or(MockEndpointError::MissingAccount)?;
        require!(
            payer.is_signer && payer.is_writable,
            MockEndpointError::InvalidPayer
        );
        emit!(MockConfigInitialized {
            oapp: params.oapp,
            eid: params.eid,
        });
        Ok(())
    }

    pub fn set_config(_ctx: Context<SetConfig>, params: SetConfigParams) -> Result<()> {
        emit!(MockConfigSet {
            oapp: params.oapp,
            eid: params.eid,
            config_type: params.config_type,
            config: params.config,
        });
        Ok(())
    }

    pub fn clear(ctx: Context<Clear>, params: ClearParams) -> Result<[u8; 32]> {
        let mut hasher = Keccak::v256();
        hasher.update(&params.guid);
        hasher.update(&params.message);
        let mut hash = [0u8; 32];
        hasher.finalize(&mut hash);
        require!(
            hash == ctx.accounts.payload_hash.hash,
            MockEndpointError::PayloadHashNotFound
        );
        Ok(hash)
    }

    #[instruction(discriminator = &SEND_DISCRIMINATOR)]
    pub fn send_packet<'info>(
        ctx: Context<'info, SendPacket<'info>>,
        params: SendParams,
    ) -> Result<MessagingReceipt> {
        require!(
            params.native_fee >= MOCK_NATIVE_FEE,
            MockEndpointError::InsufficientFee
        );
        // ULN `send` tail: [uln, send_config, default_send_config, payer, treasury, system_program, ..]
        let [_uln, _send_config, _default_send_config, payer, treasury, system_program, ..] =
            ctx.remaining_accounts
        else {
            return err!(MockEndpointError::MissingAccount);
        };
        invoke(
            &system_instruction::transfer(payer.key, treasury.key, MOCK_NATIVE_FEE),
            &[payer.clone(), treasury.clone(), system_program.clone()],
        )?;

        let nonce = &mut ctx.accounts.nonce;
        nonce.outbound_nonce += 1;
        emit!(MockPacketSent {
            sender: ctx.accounts.sender.key(),
            dst_eid: params.dst_eid,
            receiver: params.receiver,
            message: params.message,
            options: params.options,
            native_fee: params.native_fee,
            nonce: nonce.outbound_nonce,
        });

        Ok(MessagingReceipt {
            guid: [0; 32],
            nonce: nonce.outbound_nonce,
            fee: MessagingFee {
                native_fee: MOCK_NATIVE_FEE,
                lz_token_fee: 0,
            },
        })
    }

    pub fn quote<'info>(
        ctx: Context<'info, Quote<'info>>,
        _params: QuoteParams,
    ) -> Result<MessagingFee> {
        require!(
            ctx.remaining_accounts
                .iter()
                .all(|account| !account.is_writable),
            MockEndpointError::WritableAccountNotAllowed
        );
        Ok(MessagingFee {
            native_fee: MOCK_NATIVE_FEE,
            lz_token_fee: 0,
        })
    }
}

#[account]
#[derive(InitSpace)]
pub struct EndpointSettings {
    pub eid: u32,
    pub bump: u8,
    pub admin: Pubkey,
    pub lz_token_mint: Option<Pubkey>,
}

#[account]
#[derive(InitSpace)]
pub struct OAppRegistry {
    pub delegate: Pubkey,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Nonce {
    pub bump: u8,
    pub outbound_nonce: u64,
    pub inbound_nonce: u64,
}

#[account]
#[derive(InitSpace)]
pub struct PendingInboundNonce {
    #[max_len(256)]
    pub nonces: Vec<u64>,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct PayloadHash {
    pub hash: [u8; 32],
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct SendLibraryConfig {
    pub message_lib: Pubkey,
    pub bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone)]
pub struct ReceiveLibraryTimeout {
    pub message_lib: Pubkey,
    pub expiry: u64,
}

#[account]
#[derive(InitSpace)]
pub struct ReceiveLibraryConfig {
    pub message_lib: Pubkey,
    pub timeout: Option<ReceiveLibraryTimeout>,
    pub bump: u8,
}

/// Not used by the mock's instructions; mirrored so tests can stage the
/// account the real endpoint reads in `set_*_library`.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, PartialEq)]
pub enum MessageLibType {
    Send,
    Receive,
    SendAndReceive,
}

#[account]
#[derive(InitSpace)]
pub struct MessageLibInfo {
    pub message_lib_type: MessageLibType,
    pub bump: u8,
    pub message_lib_bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct MockVerifyParams {
    pub receiver: Pubkey,
    pub src_eid: u32,
    pub sender: [u8; 32],
    pub nonce: u64,
    pub payload_hash: [u8; 32],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct RegisterOAppParams {
    pub delegate: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct InitNonceParams {
    pub local_oapp: Pubkey,
    pub remote_eid: u32,
    pub remote_oapp: [u8; 32],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct InitSendLibraryParams {
    pub sender: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct InitReceiveLibraryParams {
    pub receiver: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SetSendLibraryParams {
    pub sender: Pubkey,
    pub eid: u32,
    pub new_lib: Pubkey,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SetReceiveLibraryParams {
    pub receiver: Pubkey,
    pub eid: u32,
    pub new_lib: Pubkey,
    pub grace_period: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct InitConfigParams {
    pub oapp: Pubkey,
    pub eid: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SetConfigParams {
    pub oapp: Pubkey,
    pub eid: u32,
    pub config_type: u32,
    pub config: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ClearParams {
    pub receiver: Pubkey,
    pub src_eid: u32,
    pub sender: [u8; 32],
    pub nonce: u64,
    pub guid: [u8; 32],
    pub message: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SendParams {
    pub dst_eid: u32,
    pub receiver: [u8; 32],
    pub message: Vec<u8>,
    pub options: Vec<u8>,
    pub native_fee: u64,
    pub lz_token_fee: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct QuoteParams {
    pub sender: Pubkey,
    pub dst_eid: u32,
    pub receiver: [u8; 32],
    pub message: Vec<u8>,
    pub options: Vec<u8>,
    pub pay_in_lz_token: bool,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct MessagingFee {
    pub native_fee: u64,
    pub lz_token_fee: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct MessagingReceipt {
    pub guid: [u8; 32],
    pub nonce: u64,
    pub fee: MessagingFee,
}

#[event]
pub struct MockConfigInitialized {
    pub oapp: Pubkey,
    pub eid: u32,
}

#[event]
pub struct MockConfigSet {
    pub oapp: Pubkey,
    pub eid: u32,
    pub config_type: u32,
    pub config: Vec<u8>,
}

#[event]
pub struct MockPacketSent {
    pub sender: Pubkey,
    pub dst_eid: u32,
    pub receiver: [u8; 32],
    pub message: Vec<u8>,
    pub options: Vec<u8>,
    pub native_fee: u64,
    pub nonce: u64,
}

#[error_code]
pub enum MockEndpointError {
    Unauthorized,
    SameValue,
    ReadOnlyAccount,
    InvalidNonce,
    PayloadHashNotFound,
    InsufficientFee,
    MissingAccount,
    InvalidPayer,
    WritableAccountNotAllowed,
}

#[derive(Accounts)]
pub struct MockInit<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(init, payer = payer, space = 8 + EndpointSettings::INIT_SPACE, seeds = [ENDPOINT_SEED], bump)]
    pub endpoint: Account<'info, EndpointSettings>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(params: MockVerifyParams)]
pub struct MockVerify<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        seeds = [NONCE_SEED, params.receiver.as_ref(), &params.src_eid.to_be_bytes(), &params.sender[..]],
        bump = nonce.bump
    )]
    pub nonce: Account<'info, Nonce>,
    #[account(
        init,
        payer = payer,
        space = 8 + PayloadHash::INIT_SPACE,
        seeds = [
            PAYLOAD_HASH_SEED,
            params.receiver.as_ref(),
            &params.src_eid.to_be_bytes(),
            &params.sender[..],
            &params.nonce.to_be_bytes()
        ],
        bump
    )]
    pub payload_hash: Account<'info, PayloadHash>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
#[derive(Accounts)]
pub struct RegisterOApp<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub oapp: Signer<'info>,
    #[account(init, payer = payer, space = 8 + OAppRegistry::INIT_SPACE, seeds = [OAPP_SEED, oapp.key().as_ref()], bump)]
    pub oapp_registry: Account<'info, OAppRegistry>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(params: InitNonceParams)]
pub struct InitNonce<'info> {
    #[account(mut)]
    pub delegate: Signer<'info>,
    #[account(seeds = [OAPP_SEED, params.local_oapp.as_ref()], bump = oapp_registry.bump, has_one = delegate)]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        init,
        payer = delegate,
        space = 8 + Nonce::INIT_SPACE,
        seeds = [NONCE_SEED, params.local_oapp.as_ref(), &params.remote_eid.to_be_bytes(), &params.remote_oapp[..]],
        bump
    )]
    pub nonce: Account<'info, Nonce>,
    #[account(
        init,
        payer = delegate,
        space = 8 + PendingInboundNonce::INIT_SPACE,
        seeds = [PENDING_NONCE_SEED, params.local_oapp.as_ref(), &params.remote_eid.to_be_bytes(), &params.remote_oapp[..]],
        bump
    )]
    pub pending_inbound_nonce: Account<'info, PendingInboundNonce>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(params: InitSendLibraryParams)]
pub struct InitSendLibrary<'info> {
    #[account(mut)]
    pub delegate: Signer<'info>,
    #[account(seeds = [OAPP_SEED, params.sender.as_ref()], bump = oapp_registry.bump, has_one = delegate)]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        init,
        payer = delegate,
        space = 8 + SendLibraryConfig::INIT_SPACE,
        seeds = [SEND_LIBRARY_CONFIG_SEED, params.sender.as_ref(), &params.eid.to_be_bytes()],
        bump
    )]
    pub send_library_config: Account<'info, SendLibraryConfig>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(params: InitReceiveLibraryParams)]
pub struct InitReceiveLibrary<'info> {
    #[account(mut)]
    pub delegate: Signer<'info>,
    #[account(seeds = [OAPP_SEED, params.receiver.as_ref()], bump = oapp_registry.bump, has_one = delegate)]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        init,
        payer = delegate,
        space = 8 + ReceiveLibraryConfig::INIT_SPACE,
        seeds = [RECEIVE_LIBRARY_CONFIG_SEED, params.receiver.as_ref(), &params.eid.to_be_bytes()],
        bump
    )]
    pub receive_library_config: Account<'info, ReceiveLibraryConfig>,
    pub system_program: Program<'info, System>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(params: SetSendLibraryParams)]
pub struct SetSendLibrary<'info> {
    pub signer: Signer<'info>,
    #[account(
        seeds = [OAPP_SEED, params.sender.as_ref()],
        bump = oapp_registry.bump,
        constraint = signer.key() == params.sender || signer.key() == oapp_registry.delegate @ MockEndpointError::Unauthorized
    )]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        mut,
        seeds = [SEND_LIBRARY_CONFIG_SEED, params.sender.as_ref(), &params.eid.to_be_bytes()],
        bump = send_library_config.bump,
        constraint = send_library_config.message_lib != params.new_lib @ MockEndpointError::SameValue
    )]
    pub send_library_config: Account<'info, SendLibraryConfig>,
    /// CHECK: address only — the mock keeps no library registry
    #[account(seeds = [MESSAGE_LIB_SEED, params.new_lib.as_ref()], bump)]
    pub message_lib_info: UncheckedAccount<'info>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(params: SetReceiveLibraryParams)]
pub struct SetReceiveLibrary<'info> {
    pub signer: Signer<'info>,
    #[account(
        seeds = [OAPP_SEED, params.receiver.as_ref()],
        bump = oapp_registry.bump,
        constraint = signer.key() == params.receiver || signer.key() == oapp_registry.delegate @ MockEndpointError::Unauthorized
    )]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        mut,
        seeds = [RECEIVE_LIBRARY_CONFIG_SEED, params.receiver.as_ref(), &params.eid.to_be_bytes()],
        bump = receive_library_config.bump,
        constraint = receive_library_config.message_lib != params.new_lib @ MockEndpointError::SameValue
    )]
    pub receive_library_config: Account<'info, ReceiveLibraryConfig>,
    /// CHECK: address only — the mock keeps no library registry
    #[account(seeds = [MESSAGE_LIB_SEED, params.new_lib.as_ref()], bump)]
    pub message_lib_info: UncheckedAccount<'info>,
}

#[derive(Accounts)]
#[instruction(params: InitConfigParams)]
pub struct InitConfig<'info> {
    pub delegate: Signer<'info>,
    #[account(seeds = [OAPP_SEED, params.oapp.as_ref()], bump = oapp_registry.bump, has_one = delegate)]
    pub oapp_registry: Account<'info, OAppRegistry>,
    /// CHECK: the real endpoint signs into the library with it; must be read-only
    #[account(constraint = !message_lib_info.is_writable @ MockEndpointError::ReadOnlyAccount)]
    pub message_lib_info: UncheckedAccount<'info>,
    /// CHECK: ignored by the mock
    pub message_lib: UncheckedAccount<'info>,
    /// CHECK: ignored by the mock
    pub message_lib_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
#[instruction(params: SetConfigParams)]
pub struct SetConfig<'info> {
    pub signer: Signer<'info>,
    #[account(
        seeds = [OAPP_SEED, params.oapp.as_ref()],
        bump = oapp_registry.bump,
        constraint = signer.key() == params.oapp || signer.key() == oapp_registry.delegate @ MockEndpointError::Unauthorized
    )]
    pub oapp_registry: Account<'info, OAppRegistry>,
    /// CHECK: must be read-only, as on the real endpoint
    #[account(constraint = !message_lib_info.is_writable @ MockEndpointError::ReadOnlyAccount)]
    pub message_lib_info: UncheckedAccount<'info>,
    /// CHECK: ignored by the mock
    pub message_lib: UncheckedAccount<'info>,
    /// CHECK: ignored by the mock
    pub message_lib_program: UncheckedAccount<'info>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(params: ClearParams)]
pub struct Clear<'info> {
    pub signer: Signer<'info>,
    #[account(
        seeds = [OAPP_SEED, params.receiver.as_ref()],
        bump = oapp_registry.bump,
        constraint = signer.key() == params.receiver || signer.key() == oapp_registry.delegate @ MockEndpointError::Unauthorized
    )]
    pub oapp_registry: Account<'info, OAppRegistry>,
    #[account(
        seeds = [NONCE_SEED, params.receiver.as_ref(), &params.src_eid.to_be_bytes(), &params.sender[..]],
        bump = nonce.bump,
        constraint = params.nonce <= nonce.inbound_nonce @ MockEndpointError::InvalidNonce
    )]
    pub nonce: Account<'info, Nonce>,
    #[account(
        mut,
        seeds = [
            PAYLOAD_HASH_SEED,
            params.receiver.as_ref(),
            &params.src_eid.to_be_bytes(),
            &params.sender[..],
            &params.nonce.to_be_bytes()
        ],
        bump = payload_hash.bump,
        close = endpoint
    )]
    pub payload_hash: Account<'info, PayloadHash>,
    #[account(mut, seeds = [ENDPOINT_SEED], bump = endpoint.bump)]
    pub endpoint: Account<'info, EndpointSettings>,
}

#[event_cpi]
#[derive(Accounts)]
#[instruction(params: SendParams)]
pub struct SendPacket<'info> {
    pub sender: Signer<'info>,
    /// CHECK: the real endpoint asserts it is the configured send library
    pub send_library_program: UncheckedAccount<'info>,
    #[account(
        seeds = [SEND_LIBRARY_CONFIG_SEED, sender.key().as_ref(), &params.dst_eid.to_be_bytes()],
        bump = send_library_config.bump
    )]
    pub send_library_config: Account<'info, SendLibraryConfig>,
    /// CHECK: LayerZero-admin default; not staged by the mock
    pub default_send_library_config: UncheckedAccount<'info>,
    /// CHECK: must be read-only, as on the real endpoint
    #[account(constraint = !send_library_info.is_writable @ MockEndpointError::ReadOnlyAccount)]
    pub send_library_info: UncheckedAccount<'info>,
    #[account(seeds = [ENDPOINT_SEED], bump = endpoint.bump)]
    pub endpoint: Account<'info, EndpointSettings>,
    #[account(
        mut,
        seeds = [NONCE_SEED, sender.key().as_ref(), &params.dst_eid.to_be_bytes(), &params.receiver[..]],
        bump = nonce.bump
    )]
    pub nonce: Account<'info, Nonce>,
}

#[derive(Accounts)]
#[instruction(params: QuoteParams)]
pub struct Quote<'info> {
    /// CHECK: the real endpoint asserts it is the configured send library
    pub send_library_program: UncheckedAccount<'info>,
    #[account(
        seeds = [SEND_LIBRARY_CONFIG_SEED, params.sender.as_ref(), &params.dst_eid.to_be_bytes()],
        bump = send_library_config.bump
    )]
    pub send_library_config: Account<'info, SendLibraryConfig>,
    /// CHECK: LayerZero-admin default; not staged by the mock
    pub default_send_library_config: UncheckedAccount<'info>,
    /// CHECK: must be read-only, as on the real endpoint
    #[account(constraint = !send_library_info.is_writable @ MockEndpointError::ReadOnlyAccount)]
    pub send_library_info: UncheckedAccount<'info>,
    #[account(seeds = [ENDPOINT_SEED], bump = endpoint.bump)]
    pub endpoint: Account<'info, EndpointSettings>,
    #[account(
        seeds = [NONCE_SEED, params.sender.as_ref(), &params.dst_eid.to_be_bytes(), &params.receiver[..]],
        bump = nonce.bump
    )]
    pub nonce: Account<'info, Nonce>,
}
