use anchor_lang::{AnchorDeserialize, InstructionData, ToAccountMetas};
use anyhow::Result;
use eco_routes::{
    instructions::{
        dispatch_authority_key, execution_authority_key, ClaimIntentNativeArgs, ClaimIntentSplArgs,
        FulfillIntentArgs, FundIntentNativeArgs, FundIntentSplArgs, PublishIntentArgs,
        SerializableAccountMeta, SvmCallData,
    },
    state::{Call, Intent, IntentFulfillmentMarker, IntentStatus, Reward, Route, TokenAmount},
};
use litesvm::LiteSVM;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_sdk::{account::Account, program_pack::Pack, pubkey::Pubkey};
use solana_sdk::{compute_budget::ComputeBudgetInstruction, pubkey};
use solana_signer::Signer as _;
use solana_transaction::Transaction;
use tiny_keccak::{Hasher, Keccak};

pub mod spl_noop {
    use anchor_lang::declare_id;

    declare_id!("noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV");
}

use crate::{
    helpers::{self, sol_amount, usdc_amount},
    multisig_ism_stub, utils,
};

pub const TX_FEE_AMOUNT: u64 = 5_000;

#[test]
/**
 * E2E test from SVM to SVM
 *
 * - The intent of the user we are testing on is a bridge of 5 USDC on SVM1 to 5 USDC on SVM2.
 * - The user is sponsoring all the fees on the source chain for the user.
 * - The user is offering 0.03 SOL on SVM1 as a reward for the solver.
 *
 * 1. Create intent
 * 2. Fund intent
 * 3. Solver detects intent
 * 4. Fulfillment flow
 * 5. Claim intent
 * 6. Close intent
 *
*/
fn svm_to_svm_e2e() -> Result<()> {
    let mut source_svm = utils::init_svm();
    let mut target_svm = utils::init_svm();
    let mut context = initialize_context(&mut source_svm, &mut target_svm)?;

    // std::thread::sleep(std::time::Duration::from_secs(5));

    println!("creating intent");

    create_intent(&mut context)?;

    // std::thread::sleep(std::time::Duration::from_secs(5));

    println!("funding intent");

    fund_intent(&mut context)?;

    // std::thread::sleep(std::time::Duration::from_secs(5));

    println!("solving intent");

    solve_intent(&mut context)?;

    std::thread::sleep(std::time::Duration::from_secs(5));

    println!("claiming intent");

    claim_intent(&mut context)?;

    std::thread::sleep(std::time::Duration::from_secs(5));

    println!("closing intent");

    close_intent(&mut context)?;

    Ok(())
}

struct Context<'a> {
    pub source_svm: &'a mut LiteSVM,
    pub destination_svm: &'a mut LiteSVM,

    // Actors
    pub fee_payer: Keypair,
    pub source_user: Keypair,
    pub destination_user: Keypair,
    pub solver: Keypair,
    pub hyperlane_relayer_source: Keypair,

    // Mint
    pub source_usdc_mint: Keypair,
    pub destination_usdc_mint: Keypair,

    // Intent
    pub intent_hash: [u8; 32],
    pub route: Route,
    pub reward: Reward,
}

fn initialize_context<'a>(
    source_svm: &'a mut LiteSVM,
    destination_svm: &'a mut LiteSVM,
) -> Result<Context<'a>> {
    // Actors

    let fee_payer = Keypair::new();
    let source_user = Keypair::new();
    let destination_user = Keypair::new();
    let solver = Keypair::new();
    let hyperlane_relayer_source = Keypair::new();

    helpers::write_account_no_data(source_svm, source_user.pubkey(), sol_amount(0.03))?;

    helpers::write_account_no_data(source_svm, fee_payer.pubkey(), sol_amount(1.0))?;
    helpers::write_account_no_data(destination_svm, fee_payer.pubkey(), sol_amount(1.0))?;

    helpers::write_account_no_data(source_svm, solver.pubkey(), sol_amount(1.0))?;
    helpers::write_account_no_data(destination_svm, solver.pubkey(), sol_amount(1.0))?;

    helpers::write_account_no_data(
        source_svm,
        hyperlane_relayer_source.pubkey(),
        sol_amount(10.0),
    )?;

    // Mint

    let source_usdc_mint = Keypair::new();
    let destination_usdc_mint = Keypair::new();

    // Initialize mint accounts

    let usdc_mint_data = &mut [0u8; spl_token::state::Mint::LEN];
    spl_token::state::Mint::pack(
        spl_token::state::Mint {
            decimals: 6,
            is_initialized: true,
            ..spl_token::state::Mint::default()
        },
        usdc_mint_data,
    )?;

    helpers::write_account_re(
        source_svm,
        source_usdc_mint.pubkey(),
        spl_token::ID,
        usdc_mint_data.to_vec(),
    )?;

    helpers::write_account_re(
        destination_svm,
        destination_usdc_mint.pubkey(),
        spl_token::ID,
        usdc_mint_data.to_vec(),
    )?;

    // Initialize token accounts

    let source_user_usdc_token_pubkey = spl_associated_token_account::get_associated_token_address(
        &source_user.pubkey(),
        &source_usdc_mint.pubkey(),
    );

    let source_user_usdc_token_data = &mut [0u8; spl_token::state::Account::LEN];
    spl_token::state::Account::pack(
        spl_token::state::Account {
            amount: usdc_amount(5.0),
            mint: source_usdc_mint.pubkey(),
            owner: source_user.pubkey(),
            state: spl_token::state::AccountState::Initialized,
            ..spl_token::state::Account::default()
        },
        source_user_usdc_token_data,
    )?;

    helpers::write_account_re(
        source_svm,
        source_user_usdc_token_pubkey,
        spl_token::ID,
        source_user_usdc_token_data.to_vec(),
    )?;

    let solver_user_usdc_token_pubkey = spl_associated_token_account::get_associated_token_address(
        &solver.pubkey(),
        &destination_usdc_mint.pubkey(),
    );

    let solver_usdc_token_data = &mut [0u8; spl_token::state::Account::LEN];
    spl_token::state::Account::pack(
        spl_token::state::Account {
            amount: usdc_amount(5.0),
            mint: destination_usdc_mint.pubkey(),
            owner: solver.pubkey(),
            state: spl_token::state::AccountState::Initialized,
            ..spl_token::state::Account::default()
        },
        solver_usdc_token_data,
    )?;

    helpers::write_account_re(
        destination_svm,
        solver_user_usdc_token_pubkey,
        spl_token::ID,
        solver_usdc_token_data.to_vec(),
    )?;

    // Intent parameters

    let salt = helpers::generate_salt();

    let execution_authority_pubkey = execution_authority_key(&salt).0;

    let create_ata_instruction =
        spl_associated_token_account::instruction::create_associated_token_account(
            &eco_routes::ID, // solver is represent by a default pubkey, because it is unknown at this point
            &destination_user.pubkey(),
            &destination_usdc_mint.pubkey(),
            &spl_token::ID,
        );

    let mut transfer_instruction = spl_token::instruction::transfer(
        &spl_token::ID,
        &spl_associated_token_account::get_associated_token_address(
            &execution_authority_pubkey,
            &destination_usdc_mint.pubkey(),
        ),
        &spl_associated_token_account::get_associated_token_address(
            &destination_user.pubkey(),
            &destination_usdc_mint.pubkey(),
        ),
        &execution_authority_pubkey,
        &[],
        usdc_amount(5.0),
    )?;

    transfer_instruction.accounts.iter_mut().for_each(|a| {
        if a.pubkey == execution_authority_pubkey {
            a.is_writable = true;
        }
    });

    let route = Route {
        salt,
        source_domain_id: 1,
        destination_domain_id: 1,
        inbox: eco_routes::ID.to_bytes(),
        tokens: vec![TokenAmount {
            token: destination_usdc_mint.pubkey().to_bytes(),
            amount: usdc_amount(5.0),
        }],
        calls: vec![
            Call {
                destination: spl_associated_token_account::ID.to_bytes(),
                calldata: SvmCallData {
                    instruction_data: create_ata_instruction.data,
                    num_account_metas: create_ata_instruction.accounts.len() as u8,
                    account_metas: create_ata_instruction
                        .accounts
                        .clone()
                        .into_iter()
                        .map(SerializableAccountMeta::from)
                        .collect(),
                }
                .to_bytes()?,
            },
            Call {
                destination: spl_token::ID.to_bytes(),
                calldata: {
                    let c = SvmCallData {
                        instruction_data: transfer_instruction.data,
                        num_account_metas: transfer_instruction.accounts.len() as u8,
                        account_metas: transfer_instruction
                            .accounts
                            .into_iter()
                            .map(SerializableAccountMeta::from)
                            .collect(),
                    };
                    println!("c: {:?}", c);
                    c
                }
                .to_bytes()?,
            },
        ],
    };

    let reward = Reward {
        creator: source_user.pubkey(),
        prover: eco_routes::hyperlane::MAILBOX_ID.to_bytes(),
        tokens: vec![TokenAmount {
            token: source_usdc_mint.pubkey().to_bytes(),
            amount: usdc_amount(5.0),
        }],
        native_amount: sol_amount(0.03),
        deadline: 0,
    };

    let intent_hash = eco_routes::encoding::get_intent_hash(&route, &reward);

    Ok(Context {
        source_svm,
        destination_svm,
        fee_payer,
        source_user,
        destination_user,
        solver,
        source_usdc_mint,
        destination_usdc_mint,
        intent_hash,
        route,
        reward,
        hyperlane_relayer_source,
    })
}

fn create_intent(context: &mut Context) -> Result<()> {
    let publish_intent_args = eco_routes::instruction::PublishIntent {
        args: PublishIntentArgs {
            salt: context.route.salt,
            intent_hash: context.intent_hash,
            destination_domain_id: context.route.destination_domain_id,
            inbox: context.route.inbox,
            route_tokens: context.route.tokens.clone(),
            calls: context.route.calls.clone(),
            reward_tokens: context.reward.tokens.clone(),
            native_reward: context.reward.native_amount,
            deadline: context.reward.deadline,
        },
    };

    let transaction = Transaction::new(
        &[&context.fee_payer, &context.source_user],
        Message::new(
            &[Instruction {
                program_id: eco_routes::ID,
                accounts: eco_routes::accounts::PublishIntent {
                    intent: Intent::pda(context.intent_hash).0,
                    creator: context.source_user.pubkey(),
                    payer: context.fee_payer.pubkey(),
                    system_program: solana_system_interface::program::ID,
                }
                .to_account_metas(None),
                data: publish_intent_args.data(),
            }],
            Some(&context.fee_payer.pubkey()),
        ),
        context.source_svm.latest_blockhash(),
    );

    context
        .source_svm
        .send_transaction(transaction)
        .map_err(|e| anyhow::anyhow!("Failed to send transaction: {:?}", e))?;

    let intent = helpers::read_account_anchor::<Intent>(
        context.source_svm,
        &Intent::pda(context.intent_hash).0,
    )?;

    let expected_intent = Intent {
        intent_hash: context.intent_hash,
        status: IntentStatus::Initialized,
        route: context.route.clone(),
        reward: context.reward.clone(),
        tokens_funded: 0,
        native_funded: false,
        solver: Pubkey::default().to_bytes(),
        bump: Intent::pda(context.intent_hash).1,
    };

    assert_eq!(intent, expected_intent, "Malformed Intent account state");

    Ok(())
}

fn fund_intent(context: &mut Context) -> Result<()> {
    let fund_intent_native_args = eco_routes::instruction::FundIntentNative {
        args: FundIntentNativeArgs {
            intent_hash: context.intent_hash,
        },
    };

    let fund_intent_spl_args = eco_routes::instruction::FundIntentSpl {
        args: FundIntentSplArgs {
            intent_hash: context.intent_hash,
            token_to_fund: 0,
        },
    };

    let transaction = Transaction::new(
        &[&context.fee_payer, &context.source_user],
        Message::new(
            &[
                Instruction {
                    program_id: eco_routes::ID,
                    accounts: eco_routes::accounts::FundIntentNative {
                        intent: Intent::pda(context.intent_hash).0,
                        funder: context.source_user.pubkey(),
                        payer: context.fee_payer.pubkey(),
                        system_program: solana_system_interface::program::ID,
                    }
                    .to_account_metas(None),
                    data: fund_intent_native_args.data(),
                },
                Instruction {
                    program_id: eco_routes::ID,
                    accounts: eco_routes::accounts::FundIntentSpl {
                        intent: Intent::pda(context.intent_hash).0,
                        funder: context.source_user.pubkey(),
                        payer: context.fee_payer.pubkey(),
                        system_program: solana_system_interface::program::ID,
                        source_token: spl_associated_token_account::get_associated_token_address(
                            &context.source_user.pubkey(),
                            &context.source_usdc_mint.pubkey(),
                        ),
                        destination_token: Pubkey::find_program_address(
                            &[
                                b"reward",
                                context.intent_hash.as_ref(),
                                context.source_usdc_mint.pubkey().as_ref(),
                            ],
                            &eco_routes::ID,
                        )
                        .0,
                        mint: context.source_usdc_mint.pubkey(),
                        token_program: spl_token::ID,
                    }
                    .to_account_metas(None),
                    data: fund_intent_spl_args.data(),
                },
            ],
            Some(&context.fee_payer.pubkey()),
        ),
        context.source_svm.latest_blockhash(),
    );

    let result = context.source_svm.send_transaction(transaction);

    match &result {
        Ok(_) => println!("funding intent success"),
        Err(e) => {
            for log in e.meta.logs.iter() {
                println!("{:?}", log);
            }
        }
    };

    result.map_err(|e| anyhow::anyhow!("Failed to send transaction: {:?}", e))?;

    let intent = helpers::read_account_anchor::<Intent>(
        context.source_svm,
        &Intent::pda(context.intent_hash).0,
    )?;

    assert_eq!(
        intent.status,
        IntentStatus::Funded,
        "Intent status should be funded"
    );
    assert_eq!(
        intent.native_funded, true,
        "Native funded flag should be true"
    );
    assert_eq!(
        intent.tokens_funded as usize,
        intent.reward.tokens.len(),
        "Tokens funded count should be equal to the size of the reward token array"
    );

    Ok(())
}

fn solve_intent(context: &mut Context) -> Result<()> {
    let (outbox_pda, _) = Pubkey::find_program_address(
        &[b"hyperlane", b"-", b"outbox"],
        &eco_routes::hyperlane::MAILBOX_ID,
    );

    // unique-message account for OutboxDispatch
    let unique_message = Keypair::new();

    let (dispatched_message_pda, _) = Pubkey::find_program_address(
        &[
            b"hyperlane",
            b"-",
            b"dispatched_message",
            b"-",
            unique_message.pubkey().as_ref(),
        ],
        &eco_routes::hyperlane::MAILBOX_ID,
    );

    // strip account-metas from calldata (optimization for tx size)
    let mut route_without_metas = context.route.clone();
    route_without_metas.calls.iter_mut().for_each(|c| {
        let stub = SvmCallData::from_calldata_without_account_metas(&c.calldata).unwrap();
        c.calldata = stub.to_bytes().unwrap();
    });

    let initialize_ata_ixs = context
        .route
        .tokens
        .iter()
        .map(|t| {
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &context.solver.pubkey(),
                &execution_authority_key(&context.route.salt).0,
                &Pubkey::new_from_array(t.token),
                &spl_token::ID,
            )
        })
        .collect::<Vec<_>>();

    let fulfill_ix = Instruction {
        program_id: eco_routes::ID,
        accounts: eco_routes::accounts::FulfillIntent {
            payer: context.solver.pubkey(),
            solver: context.solver.pubkey(),
            execution_authority: execution_authority_key(&context.route.salt).0,
            dispatch_authority: dispatch_authority_key().0,
            mailbox_program: eco_routes::hyperlane::MAILBOX_ID,
            outbox_pda,
            spl_noop_program: spl_noop::id(),
            unique_message: unique_message.pubkey(),
            intent_fulfillment_marker: IntentFulfillmentMarker::pda(context.intent_hash).0,
            dispatched_message_pda,
            spl_token_program: spl_token::ID,
            spl_token_2022_program: spl_token_2022::ID,
            system_program: solana_system_interface::program::ID,
        }
        .to_account_metas(None)
        .into_iter()
        .chain({
            context
                .route
                .tokens
                .iter()
                .map(|t| {
                    vec![
                        AccountMeta {
                            pubkey: Pubkey::new_from_array(t.token),
                            is_signer: false,
                            is_writable: false,
                        },
                        AccountMeta {
                            pubkey: spl_associated_token_account::get_associated_token_address(
                                &context.solver.pubkey(),
                                &Pubkey::new_from_array(t.token),
                            ),
                            is_signer: false,
                            is_writable: true,
                        },
                        AccountMeta {
                            pubkey: spl_associated_token_account::get_associated_token_address(
                                &execution_authority_key(&context.route.salt).0,
                                &Pubkey::new_from_array(t.token),
                            ),
                            is_signer: false,
                            is_writable: true,
                        },
                    ]
                })
                .flatten()
        })
        // add the SVM-call account metas we stripped out earlier - they are needed in metas but not in data
        .chain({
            context.route.calls.iter().flat_map(|c| {
                SvmCallData::try_from_slice(&c.calldata)
                    .unwrap()
                    .account_metas
                    .into_iter()
                    .map(|m: SerializableAccountMeta| AccountMeta {
                        pubkey: if m.pubkey == eco_routes::ID {
                            context.solver.pubkey()
                        } else {
                            m.pubkey
                        },
                        is_signer: if m.pubkey == execution_authority_key(&context.route.salt).0 {
                            false
                        } else {
                            m.is_signer
                        },
                        is_writable: if m.pubkey == execution_authority_key(&context.route.salt).0 {
                            true
                        } else {
                            m.is_writable
                        },
                    })
            })
        })
        .collect::<Vec<_>>(),
        data: eco_routes::instruction::FulfillIntent {
            args: FulfillIntentArgs {
                intent_hash: context.intent_hash,
                route: route_without_metas,
                reward: context.reward.clone(),
            },
        }
        .data(),
    };

    println!("fulfill_ix: {:#?}", fulfill_ix.accounts);

    let compute_budget_ix = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);

    let instructions = initialize_ata_ixs
        .into_iter()
        .chain([compute_budget_ix])
        .chain([fulfill_ix])
        .collect::<Vec<_>>();

    let fulfill_tx = Transaction::new(
        &[&context.solver, &unique_message],
        Message::new(&instructions, Some(&context.solver.pubkey())),
        context.destination_svm.latest_blockhash(),
    );

    context
        .destination_svm
        .send_transaction(fulfill_tx)
        .map_err(|e| anyhow::anyhow!("Failed to send transaction: {:?}", e))?;

    let dispatch_account = context
        .destination_svm
        .get_account(&dispatched_message_pda)
        .ok_or_else(|| anyhow::anyhow!("dispatched message account missing"))?;

    let bytes = dispatch_account.data;

    let message_bytes = bytes[53..].to_vec();

    let mut hasher = Keccak::v256();
    hasher.update(&message_bytes);
    let mut id = [0u8; 32];
    hasher.finalize(&mut id);

    let (processed_message_pda, _) = Pubkey::find_program_address(
        &[b"hyperlane", b"-", b"processed_message", b"-", &id],
        &eco_routes::hyperlane::MAILBOX_ID,
    );
    let (inbox_pda, _) = Pubkey::find_program_address(
        &[b"hyperlane", b"-", b"inbox"],
        &eco_routes::hyperlane::MAILBOX_ID,
    );
    let (process_authority_pda, _) = Pubkey::find_program_address(
        &[
            b"hyperlane",
            b"-",
            b"process_authority",
            b"-",
            eco_routes::ID.as_ref(),
        ],
        &eco_routes::hyperlane::MAILBOX_ID,
    );

    println!("process_authority_pda: {}", process_authority_pda);

    let mut ix_data = vec![1u8]; // enum tag: InboxProcess
    ix_data.extend_from_slice(&0u32.to_le_bytes()); // empty metadata vec
    ix_data.extend_from_slice(&(message_bytes.len() as u32).to_le_bytes());
    ix_data.extend_from_slice(&message_bytes);

    let inbox_process_ix = Instruction {
        program_id: eco_routes::hyperlane::MAILBOX_ID,
        accounts: vec![
            // 0-4 – core mailbox accounts
            AccountMeta::new(context.hyperlane_relayer_source.pubkey(), true),
            AccountMeta::new_readonly(solana_system_interface::program::ID, false),
            AccountMeta::new(inbox_pda, false),
            AccountMeta::new_readonly(process_authority_pda, false),
            AccountMeta::new(processed_message_pda, false),
            // 5..N – recipient’s ISM-meta account(s)
            AccountMeta::new(
                Pubkey::find_program_address(
                    &[
                        b"hyperlane_message_recipient",
                        b"-",
                        b"interchain_security_module",
                        b"-",
                        b"account_metas",
                    ],
                    &eco_routes::ID,
                )
                .0,
                false,
            ),
            // N+1 – SPL-noop
            AccountMeta::new_readonly(spl_noop::id(), false),
            // N+2 – ISM program id
            AccountMeta::new_readonly(eco_routes::hyperlane::MULTISIG_ISM_ID, false),
            // N+3..M – ISM::verify accounts (for Multisig ISM it’s just DomainData)
            AccountMeta::new(
                multisig_ism_stub::domain_data_pda(
                    eco_routes::hyperlane::DOMAIN_ID,
                    &eco_routes::hyperlane::MULTISIG_ISM_ID,
                )
                .0,
                false,
            ),
            // M+1 – recipient program id
            AccountMeta::new_readonly(eco_routes::ID, false),
            // M+2..K – recipient::handle accounts
            AccountMeta::new(Intent::pda(context.intent_hash).0, false),
        ],
        data: ix_data,
    };

    let inbox_tx = Transaction::new(
        &[&context.hyperlane_relayer_source],
        Message::new(
            &[inbox_process_ix],
            Some(&context.hyperlane_relayer_source.pubkey()),
        ),
        context.source_svm.latest_blockhash(),
    );

    context
        .source_svm
        .send_transaction(inbox_tx)
        .map_err(|e| anyhow::anyhow!("inbox process tx failed: {e:?}"))?;

    let intent = helpers::read_account_anchor::<Intent>(
        context.source_svm,
        &Intent::pda(context.intent_hash).0,
    )?;
    assert_eq!(
        intent.status,
        IntentStatus::Fulfilled,
        "intent should be Fulfilled after inbox processing"
    );

    Ok(())
}

fn claim_intent(context: &mut Context) -> Result<()> {
    let claim_intent_native_args = eco_routes::instruction::ClaimIntentNative {
        args: ClaimIntentNativeArgs {
            intent_hash: context.intent_hash,
        },
    };

    let claim_intent_spl_args = eco_routes::instruction::ClaimIntentSpl {
        args: ClaimIntentSplArgs {
            intent_hash: context.intent_hash,
            token_to_claim: 0,
        },
    };

    let transaction = Transaction::new(
        &[&context.solver],
        Message::new(
            &[
                Instruction {
                    program_id: eco_routes::ID,
                    accounts: eco_routes::accounts::ClaimIntentNative {
                        intent: Intent::pda(context.intent_hash).0,
                        claimer: context.solver.pubkey(),
                        payer: context.solver.pubkey(),
                        system_program: solana_system_interface::program::ID,
                    }
                    .to_account_metas(None),
                    data: claim_intent_native_args.data(),
                },
                Instruction {
                    program_id: eco_routes::ID,
                    accounts: eco_routes::accounts::ClaimIntentSpl {
                        intent: Intent::pda(context.intent_hash).0,
                        claimer: context.solver.pubkey(),
                        payer: context.solver.pubkey(),
                        system_program: solana_system_interface::program::ID,
                        source_token: spl_associated_token_account::get_associated_token_address(
                            &context.solver.pubkey(),
                            &context.source_usdc_mint.pubkey(),
                        ),
                        destination_token: Pubkey::find_program_address(
                            &[b"reward", context.source_usdc_mint.pubkey().as_ref()],
                            &eco_routes::ID,
                        )
                        .0,
                        mint: context.source_usdc_mint.pubkey(),
                        token_program: spl_token::ID,
                    }
                    .to_account_metas(None),
                    data: claim_intent_spl_args.data(),
                },
            ],
            Some(&context.solver.pubkey()),
        ),
        context.source_svm.latest_blockhash(),
    );

    context
        .source_svm
        .send_transaction(transaction)
        .map_err(|e| anyhow::anyhow!("Failed to send transaction: {:?}", e))?;

    let intent = helpers::read_account_anchor::<Intent>(
        context.source_svm,
        &Intent::pda(context.intent_hash).0,
    )?;

    assert_eq!(
        intent.status,
        IntentStatus::Claimed,
        "Intent status should be claimed"
    );
    assert_eq!(
        intent.native_funded, false,
        "Native funded flag should be false"
    );
    assert_eq!(
        intent.tokens_funded as usize, 0,
        "Tokens funded count should be 0"
    );

    Ok(())
}

fn close_intent(context: &mut Context) -> Result<()> {
    let close_intent_args = eco_routes::instruction::CloseIntent;

    let transaction = Transaction::new(
        &[&context.fee_payer],
        Message::new(
            &[Instruction {
                program_id: eco_routes::ID,
                accounts: eco_routes::accounts::CloseIntent {
                    intent: Intent::pda(context.intent_hash).0,
                    payer: context.fee_payer.pubkey(),
                    system_program: solana_system_interface::program::ID,
                }
                .to_account_metas(None),
                data: close_intent_args.data(),
            }],
            Some(&context.fee_payer.pubkey()),
        ),
        context.source_svm.latest_blockhash(),
    );

    context
        .source_svm
        .send_transaction(transaction)
        .map_err(|e| anyhow::anyhow!("Failed to send transaction: {:?}", e))?;

    let intent_account = context
        .source_svm
        .get_account(&Intent::pda(context.intent_hash).0)
        .ok_or(anyhow::anyhow!("Intent account should be in the source VM"))?;

    assert_eq!(
        intent_account,
        Account {
            lamports: 0,
            data: vec![],
            owner: solana_system_interface::program::ID,
            executable: false,
            rent_epoch: 0,
            ..Account::default()
        },
        "Intent account should be closed, hence have 0 lamports, no data, and system program as owner"
    );

    Ok(())
}
