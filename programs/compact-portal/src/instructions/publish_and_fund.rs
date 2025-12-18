use anchor_lang::prelude::*;
use anchor_lang::solana_program::hash::hash;
use anchor_spl::{associated_token, token, token_2022};
use eco_svm_std::{Bytes32, CHAIN_ID};
use portal::types::{Call, Reward, Route, TokenAmount};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct PublishAndFundArgs {
    pub destination: u64,
    pub route: Route,
    pub reward: Reward,
    pub allow_partial: bool,
}

#[derive(Accounts)]
pub struct PublishAndFund<'info> {
    /// CHECK: validated as executable
    #[account(executable)]
    pub portal_program: UncheckedAccount<'info>,
    pub payer: Signer<'info>,
    #[account(mut)]
    pub funder: Signer<'info>,
    /// CHECK: address is validated
    #[account(mut)]
    pub vault: UncheckedAccount<'info>,
    pub token_program: Program<'info, token::Token>,
    pub token_2022_program: Program<'info, token_2022::Token2022>,
    pub associated_token_program: Program<'info, associated_token::AssociatedToken>,
    pub system_program: Program<'info, System>,
}

pub fn publish_and_fund_intent<'info>(
    ctx: Context<'_, '_, '_, 'info, PublishAndFund<'info>>,
    args: PublishAndFundArgs,
) -> Result<()> {
    let PublishAndFundArgs {
        destination,
        route,
        reward,
        allow_partial,
    } = args;
    let route = if destination == CHAIN_ID {
        route.try_to_vec()?
    } else {
        route_to_abi(route)
    };
    let route_hash = keccak256(&route);

    publish_intent(
        ctx.accounts.portal_program.key(),
        destination,
        route,
        reward.clone(),
    )?;
    fund_intent(&ctx, route_hash, destination, reward, allow_partial)
}

fn publish_intent(portal: Pubkey, destination: u64, route: Vec<u8>, reward: Reward) -> Result<()> {
    let args = portal::instructions::PublishArgs {
        destination,
        route,
        reward,
    };
    let data = hash(b"global:publish").to_bytes()[..8]
        .iter()
        .copied()
        .chain(args.try_to_vec()?)
        .collect::<Vec<u8>>();
    let ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: portal,
        accounts: vec![],
        data,
    };

    anchor_lang::solana_program::program::invoke(&ix, &[])?;

    Ok(())
}

fn fund_intent<'info>(
    ctx: &Context<'_, '_, '_, 'info, PublishAndFund<'info>>,
    route_hash: Bytes32,
    destination: u64,
    reward: Reward,
    allow_partial: bool,
) -> Result<()> {
    use anchor_lang::solana_program::instruction::AccountMeta;

    let args = portal::instructions::FundArgs {
        destination,
        route_hash,
        reward,
        allow_partial,
    };
    let data = hash(b"global:fund").to_bytes()[..8]
        .iter()
        .copied()
        .chain(args.try_to_vec()?)
        .collect::<Vec<u8>>();

    let accounts = vec![
        AccountMeta::new_readonly(ctx.accounts.payer.key(), true),
        AccountMeta::new(ctx.accounts.funder.key(), true),
        AccountMeta::new(ctx.accounts.vault.key(), false),
        AccountMeta::new_readonly(ctx.accounts.token_program.key(), false),
        AccountMeta::new_readonly(ctx.accounts.token_2022_program.key(), false),
        AccountMeta::new_readonly(ctx.accounts.associated_token_program.key(), false),
        AccountMeta::new_readonly(ctx.accounts.system_program.key(), false),
    ]
    .into_iter()
    .chain(ctx.remaining_accounts.iter().map(|acc| {
        if acc.is_writable {
            AccountMeta::new(acc.key(), acc.is_signer)
        } else {
            AccountMeta::new_readonly(acc.key(), acc.is_signer)
        }
    }))
    .collect();

    let ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: ctx.accounts.portal_program.key(),
        accounts,
        data,
    };

    let account_infos = [
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.funder.to_account_info(),
        ctx.accounts.vault.to_account_info(),
        ctx.accounts.token_program.to_account_info(),
        ctx.accounts.token_2022_program.to_account_info(),
        ctx.accounts.associated_token_program.to_account_info(),
        ctx.accounts.system_program.to_account_info(),
    ]
    .into_iter()
    .chain(ctx.remaining_accounts.iter().cloned())
    .collect::<Vec<_>>();

    anchor_lang::solana_program::program::invoke(&ix, &account_infos)?;

    Ok(())
}

fn keccak256(data: &[u8]) -> Bytes32 {
    use tiny_keccak::{Hasher, Keccak};

    let mut hasher = Keccak::v256();
    let mut hash = [0u8; 32];
    hasher.update(data);
    hasher.finalize(&mut hash);

    hash.into()
}

pub fn route_to_abi(route: Route) -> Vec<u8> {
    let tokens_offset: u64 = 6 * 32;
    let tokens_size = 32 + route.tokens.len() as u64 * 64;
    let calls_offset = tokens_offset + tokens_size;

    pad_u64(32)
        .chain(route.salt.iter().copied())
        .chain(pad_u64(route.deadline))
        .chain(bytes32_to_address(&route.portal))
        .chain(pad_u64(route.native_amount))
        .chain(pad_u64(tokens_offset))
        .chain(pad_u64(calls_offset))
        .chain(encode_token_amounts(&route.tokens))
        .chain(encode_calls(&route.calls))
        .collect()
}

fn encode_token_amounts(tokens: &[TokenAmount]) -> impl Iterator<Item = u8> + '_ {
    pad_u64(tokens.len() as u64).chain(tokens.iter().flat_map(|token| {
        pubkey_to_address(&token.token)
            .into_iter()
            .chain(pad_u64(token.amount))
    }))
}

fn encode_calls(calls: &[Call]) -> impl Iterator<Item = u8> + '_ {
    let offsets: Vec<u64> = calls
        .iter()
        .scan(calls.len() * 32, |offset, call| {
            let current = *offset as u64;
            *offset += encode_call_size(call);
            Some(current)
        })
        .collect();

    pad_u64(calls.len() as u64)
        .chain(offsets.into_iter().flat_map(pad_u64))
        .chain(calls.iter().flat_map(encode_call))
}

fn encode_call_size(call: &Call) -> usize {
    3 * 32 + 32 + (call.data.len() + 31) / 32 * 32
}

fn encode_call(call: &Call) -> impl Iterator<Item = u8> + '_ {
    let padding_needed = (32 - call.data.len() % 32) % 32;

    bytes32_to_address(&call.target)
        .into_iter()
        .chain(pad_u64(3 * 32))
        .chain([0u8; 32])
        .chain(pad_u64(call.data.len() as u64))
        .chain(call.data.iter().copied())
        .chain(std::iter::repeat(0u8).take(padding_needed))
}

fn bytes32_to_address(bytes: &Bytes32) -> [u8; 32] {
    let mut padded = [0u8; 32];
    padded[12..32].copy_from_slice(&bytes[12..32]);
    padded
}

fn pubkey_to_address(pubkey: &Pubkey) -> [u8; 32] {
    let mut padded = [0u8; 32];
    padded[12..32].copy_from_slice(&pubkey.to_bytes()[12..32]);
    padded
}

fn pad_u64(value: u64) -> impl Iterator<Item = u8> {
    [0u8; 24].into_iter().chain(value.to_be_bytes())
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Address, FixedBytes, U256};
    use alloy_sol_types::{sol, SolValue};

    use super::*;

    sol! {
        struct SolTokenAmount {
            address token;
            uint256 amount;
        }

        struct SolCall {
            address target;
            bytes data;
            uint256 value;
        }

        struct SolRoute {
            bytes32 salt;
            uint64 deadline;
            address portal;
            uint256 nativeAmount;
            SolTokenAmount[] tokens;
            SolCall[] calls;
        }
    }

    fn to_address(bytes: &[u8; 32]) -> Address {
        Address::from_slice(&bytes[12..32])
    }

    fn route_to_sol(route: &Route) -> SolRoute {
        SolRoute {
            salt: FixedBytes::from_slice(route.salt.as_slice()),
            deadline: route.deadline,
            portal: to_address(&route.portal.clone().into()),
            nativeAmount: U256::from(route.native_amount),
            tokens: route
                .tokens
                .iter()
                .map(|t| SolTokenAmount {
                    token: to_address(&t.token.to_bytes()),
                    amount: U256::from(t.amount),
                })
                .collect(),
            calls: route
                .calls
                .iter()
                .map(|c| SolCall {
                    target: to_address(&c.target.clone().into()),
                    data: c.data.clone().into(),
                    value: U256::ZERO,
                })
                .collect(),
        }
    }

    #[test]
    fn route_to_abi_single_token_single_call() {
        let route = Route {
            salt: [1u8; 32].into(),
            deadline: 1700000000,
            portal: [
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
                2, 2, 2, 2,
            ]
            .into(),
            native_amount: 1000000000000000000,
            tokens: vec![TokenAmount {
                token: Pubkey::new_from_array([
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
                    3, 3, 3, 3, 3, 3,
                ]),
                amount: 100,
            }],
            calls: vec![Call {
                target: [
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
                    4, 4, 4, 4, 4, 4,
                ]
                .into(),
                data: vec![
                    0xa9, 0x05, 0x9c, 0xbb, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 5, 5, 5, 5, 5,
                    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 100,
                ],
            }],
        };

        let expected = route_to_sol(&route).abi_encode();
        let actual = route_to_abi(route);

        assert_eq!(actual, expected);
    }

    #[test]
    fn route_to_abi_multiple_tokens_multiple_calls() {
        let route = Route {
            salt: [0xAB; 32].into(),
            deadline: 999999999,
            portal: [
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 1,
            ]
            .into(),
            native_amount: 0,
            tokens: vec![
                TokenAmount {
                    token: Pubkey::new_from_array([
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
                        0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
                        0x11, 0x11,
                    ]),
                    amount: 1000,
                },
                TokenAmount {
                    token: Pubkey::new_from_array([
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22,
                        0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22,
                        0x22, 0x22,
                    ]),
                    amount: 2000,
                },
            ],
            calls: vec![
                Call {
                    target: [
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA,
                        0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA,
                        0xAA, 0xAA,
                    ]
                    .into(),
                    data: vec![1, 2, 3, 4],
                },
                Call {
                    target: [
                        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB,
                        0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB, 0xBB,
                        0xBB, 0xBB,
                    ]
                    .into(),
                    data: vec![5, 6, 7, 8, 9, 10],
                },
            ],
        };

        let expected = route_to_sol(&route).abi_encode();
        let actual = route_to_abi(route);

        assert_eq!(actual, expected);
    }

    #[test]
    fn route_to_abi_empty_tokens_and_calls() {
        let route = Route {
            salt: [0xFF; 32].into(),
            deadline: 0,
            portal: [0u8; 32].into(),
            native_amount: u64::MAX,
            tokens: vec![],
            calls: vec![],
        };

        let expected = route_to_sol(&route).abi_encode();
        let actual = route_to_abi(route);

        assert_eq!(actual, expected);
    }

    #[test]
    fn route_to_abi_empty_call_data() {
        let route = Route {
            salt: [0x11; 32].into(),
            deadline: 12345,
            portal: [0u8; 32].into(),
            native_amount: 0,
            tokens: vec![],
            calls: vec![Call {
                target: [0u8; 32].into(),
                data: vec![],
            }],
        };

        let expected = route_to_sol(&route).abi_encode();
        let actual = route_to_abi(route);

        assert_eq!(actual, expected);
    }

    #[test]
    fn route_to_abi_exact_32_byte_call_data() {
        let route = Route {
            salt: [0x22; 32].into(),
            deadline: 99999,
            portal: [0u8; 32].into(),
            native_amount: 500,
            tokens: vec![],
            calls: vec![Call {
                target: [0u8; 32].into(),
                data: vec![0xAB; 32],
            }],
        };

        let expected = route_to_sol(&route).abi_encode();
        let actual = route_to_abi(route);

        assert_eq!(actual, expected);
    }

    #[test]
    fn route_to_abi_large_call_data() {
        let route = Route {
            salt: [0x33; 32].into(),
            deadline: 1000000,
            portal: [0u8; 32].into(),
            native_amount: 0,
            tokens: vec![],
            calls: vec![Call {
                target: [0u8; 32].into(),
                data: (0u8..=255).cycle().take(500).collect(),
            }],
        };

        let expected = route_to_sol(&route).abi_encode();
        let actual = route_to_abi(route);

        assert_eq!(actual, expected);
    }

    #[test]
    fn route_to_abi_33_byte_call_data() {
        let route = Route {
            salt: [0x44; 32].into(),
            deadline: 0,
            portal: [0u8; 32].into(),
            native_amount: 0,
            tokens: vec![],
            calls: vec![Call {
                target: [0u8; 32].into(),
                data: vec![0xCD; 33],
            }],
        };

        let expected = route_to_sol(&route).abi_encode();
        let actual = route_to_abi(route);

        assert_eq!(actual, expected);
    }
}
