use anchor_lang::{AnchorDeserialize, AnchorSerialize, InstructionData, ToAccountMetas};
use anyhow::Result;
use console::{style, Emoji};
use eco_routes::{
    hyperlane::MAILBOX_ID,
    instructions::{
        dispatch_authority_key, execution_authority_key, ClaimIntentNativeArgs, ClaimIntentSplArgs,
        FulfillIntentArgs, FundIntentNativeArgs, FundIntentSplArgs, PublishIntentArgs,
        SerializableAccountMeta, SvmCallData, SvmCallDataWithAccountMetas,
        SOLVER_PLACEHOLDER_PUBKEY,
    },
    state::{Call, Intent, IntentFulfillmentMarker, IntentStatus, Reward, Route, TokenAmount},
};
use litesvm::LiteSVM;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_sdk::{
    account::Account, compute_budget::ComputeBudgetInstruction, program_pack::Pack, pubkey::Pubkey,
};
use solana_signer::Signer as _;
use solana_transaction::Transaction;
use tiny_keccak::{Hasher, Keccak};

pub mod spl_noop {
    use anchor_lang::declare_id;

    declare_id!("noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV");
}

use crate::{
    helpers::{self, sol_amount, usdc_amount, usdc_decimals},
    utils,
};

pub const TX_FEE_AMOUNT: u64 = 5_000;

/**
 * E2E test from SVM to SVM
 *
 * - The intent of the user we are testing on is a bridge of 5 USDC on SVM1 to 5 USDC on SVM2.
 * - The payer is sponsoring all the fees on the source chain for the user.
 * - The user is offering 0.03 SOL and 5 USDC on SVM1 as a reward for the solver.
 *
 * 1. Create intent
 * 2. Fund intent
 * 3. Solver detects intent
 * 4. Fulfillment flow
 * 5. Claim intent
 * 6. Close intent
 *
*/
pub fn svm_to_svm_e2e(token_program_id: Pubkey) -> Result<()> {
    let mut source_svm = utils::init_svm();
    let mut destination_svm = utils::init_svm();
    let mut context = initialize_context(&mut source_svm, &mut destination_svm, token_program_id)?;

    println!(
        "{} {}",
        Emoji::new("🔍", ""),
        style("This is an end-to-end test of the SVM implementation of `eco-routes`.")
            .cyan()
            .bold()
    );
    println!(
        "{}",
        style("The test will (1) create an intent, (2) fund it, (3) solve it, (4) claim it, and (5) close it.").white().bold()
    );
    println!(
        "{}",
        style("The test will be run on two simulated SVMs, `source_svm` and `destination_svm`, with the following actors:").white().bold()
    );
    println!(
        "{}{}",
        style("- Fee payer: ").bold().green(),
        style("will sponsor fees for all of the user's transactions.").bold()
    );
    println!(
        "{}{}",
        style("- Source user: ").bold().green(),
        style("will create the intent and fund it.").bold(),
    );
    println!(
        "{}{}",
        style("- Destination user: ").bold().green(),
        style("will be the recipient of the intent.").bold()
    );
    println!(
        "{}{}",
        style("- Solver: ").bold().green(),
        style("will fulfill the intent.").bold()
    );

    context.snapshot_and_log()?;
    println!("{}", style("Press Enter to begin...").cyan().bold());
    let _ = std::io::stdin().read_line(&mut String::new());

    create_intent(&mut context)?;
    println!(
        "{} {}",
        Emoji::new("✨", ""),
        style("Intent created").cyan().bold()
    );

    context.snapshot_and_log()?;
    println!("{}", style("Press Enter to continue...").cyan().bold());
    let _ = std::io::stdin().read_line(&mut String::new());

    fund_intent(&mut context)?;
    println!(
        "{} {}",
        Emoji::new("✨", ""),
        style("Intent funded").cyan().bold()
    );

    context.snapshot_and_log()?;
    println!("{}", style("Press Enter to continue...").cyan().bold());
    let _ = std::io::stdin().read_line(&mut String::new());
    solve_intent(&mut context)?;
    println!(
        "{} {}",
        Emoji::new("✨", ""),
        style("Intent solved").cyan().bold()
    );

    context.snapshot_and_log()?;
    println!("{}", style("Press Enter to continue...").cyan().bold());
    let _ = std::io::stdin().read_line(&mut String::new());
    claim_intent(&mut context)?;
    println!(
        "{} {}",
        Emoji::new("✨", ""),
        style("Intent claimed").cyan().bold()
    );

    context.snapshot_and_log()?;
    println!("{}", style("Press Enter to continue...").cyan().bold());
    let _ = std::io::stdin().read_line(&mut String::new());
    close_intent(&mut context)?;
    println!(
        "{} {}",
        Emoji::new("✨", ""),
        style("Intent closed").cyan().bold()
    );

    context.snapshot_and_log()?;
    println!("{}", style("Press Enter to exit...").cyan().bold());
    let _ = std::io::stdin().read_line(&mut String::new());

    println!("{}", style("Test completed.").cyan().bold());

    Ok(())
}

struct BalancesSnapshot {
    pub source_user_lamports: u64,
    pub source_user_usdc: u64,
    pub destination_user_lamports: u64,
    pub destination_user_usdc: u64,
    pub source_solver_lamports: u64,
    pub source_solver_usdc: u64,
    pub destination_solver_lamports: u64,
    pub destination_solver_usdc: u64,
    pub source_fee_payer_lamports: u64,
    pub destination_fee_payer_lamports: u64,
    pub intent_lamports: u64,
    pub intent_usdc: u64,
}

struct Context<'a> {
    pub token_program_id: Pubkey,

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

    pub balances_snapshot: Option<BalancesSnapshot>,
}

impl<'a> Context<'a> {
    // logs the intent state, each actors lamports and usdc, and deltas

    pub fn snapshot_and_log(&mut self) -> Result<()> {
        fn format_balance(
            balance: u64,
            pre_balance: Option<u64>,
            currency: &str,
            decimals: u8,
        ) -> String {
            let formatted_balance =
                format!("{:.2}", balance as f64 / 10.0_f64.powi(decimals as i32));

            let formatted_delta = if let Some(pre_balance) = pre_balance {
                let delta: i64 = (balance as i64).saturating_sub(pre_balance as i64);
                let formatted_delta_text =
                    format!("{:.2}", delta.abs() as f64 / 10.0_f64.powi(decimals as i32));
                if delta > 0 {
                    style(format!("(+{})", formatted_delta_text)).green().bold()
                } else if delta < 0 {
                    style(format!("(-{})", formatted_delta_text)).red().bold()
                } else {
                    style(format!("(no change)")).dim().bold()
                }
            } else {
                style(format!("(no previous balance)")).dim().bold()
            };

            style(format!(
                "{} {} {}",
                formatted_balance, currency, formatted_delta
            ))
            .bold()
            .to_string()
        }

        fn get_token_balance(svm: &LiteSVM, token_address: Pubkey) -> u64 {
            svm.get_account(&token_address)
                .map(|a| {
                    spl_token_2022::state::Account::unpack(&a.data)
                        .ok()
                        .map(|a| a.amount)
                })
                .flatten()
                .unwrap_or(0)
        }

        let source_user_lamports =
            helpers::read_account_lamports(self.source_svm, &self.source_user.pubkey())
                .unwrap_or(0);
        let destination_user_lamports =
            helpers::read_account_lamports(self.destination_svm, &self.destination_user.pubkey())
                .unwrap_or(0);
        let source_solver_lamports =
            helpers::read_account_lamports(self.source_svm, &self.solver.pubkey()).unwrap_or(0);
        let destination_solver_lamports =
            helpers::read_account_lamports(self.destination_svm, &self.solver.pubkey())
                .unwrap_or(0);
        let source_fee_payer_lamports =
            helpers::read_account_lamports(self.source_svm, &self.fee_payer.pubkey()).unwrap_or(0);
        let destination_fee_payer_lamports =
            helpers::read_account_lamports(self.destination_svm, &self.fee_payer.pubkey())
                .unwrap_or(0);

        let source_user_usdc = get_token_balance(
            self.source_svm,
            spl_associated_token_account::get_associated_token_address_with_program_id(
                &self.source_user.pubkey(),
                &self.source_usdc_mint.pubkey(),
                &self.token_program_id,
            ),
        );
        let destination_user_usdc = get_token_balance(
            self.destination_svm,
            spl_associated_token_account::get_associated_token_address_with_program_id(
                &self.destination_user.pubkey(),
                &self.destination_usdc_mint.pubkey(),
                &self.token_program_id,
            ),
        );
        let source_solver_usdc = get_token_balance(
            self.source_svm,
            spl_associated_token_account::get_associated_token_address_with_program_id(
                &self.solver.pubkey(),
                &self.source_usdc_mint.pubkey(),
                &self.token_program_id,
            ),
        );
        let destination_solver_usdc = get_token_balance(
            self.destination_svm,
            spl_associated_token_account::get_associated_token_address_with_program_id(
                &self.solver.pubkey(),
                &self.destination_usdc_mint.pubkey(),
                &self.token_program_id,
            ),
        );

        let intent_lamports =
            helpers::read_account_lamports_re(self.source_svm, &Intent::pda(self.intent_hash).0)
                .unwrap_or(0);
        let intent_usdc = get_token_balance(
            self.source_svm,
            Pubkey::find_program_address(
                &[
                    b"reward",
                    self.intent_hash.as_ref(),
                    self.source_usdc_mint.pubkey().as_ref(),
                ],
                &eco_routes::ID,
            )
            .0,
        );

        println!("{}", style("Source SVM actors: ").bold().magenta());

        println!("{}", style("User: ").bold().green());
        println!(
            "{} {}",
            style("  Lamports: ").bold().green(),
            format_balance(
                source_user_lamports,
                self.balances_snapshot
                    .as_ref()
                    .map(|s| s.source_user_lamports),
                "SOL",
                9
            )
        );
        println!(
            "{} {}",
            style("  USDC: ").bold().green(),
            format_balance(
                source_user_usdc,
                self.balances_snapshot.as_ref().map(|s| s.source_user_usdc),
                "USDC",
                6
            )
        );

        println!("{}", style("Intent: ").bold().green());
        println!(
            "{} {}",
            style("  Lamports: ").bold().green(),
            format_balance(
                intent_lamports,
                self.balances_snapshot.as_ref().map(|s| s.intent_lamports),
                "SOL",
                9
            )
        );
        println!(
            "{} {}",
            style("  USDC: ").bold().green(),
            format_balance(
                intent_usdc,
                self.balances_snapshot.as_ref().map(|s| s.intent_usdc),
                "USDC",
                6
            )
        );

        println!("{}", style("Fee payer: ").bold().green());
        println!(
            "{} {}",
            style("  Lamports: ").bold().green(),
            format_balance(
                source_fee_payer_lamports,
                self.balances_snapshot
                    .as_ref()
                    .map(|s| s.source_fee_payer_lamports),
                "SOL",
                9
            )
        );

        println!("{}", style("Solver: ").bold().green());
        println!(
            "{} {}",
            style("  Lamports: ").bold().green(),
            format_balance(
                source_solver_lamports,
                self.balances_snapshot
                    .as_ref()
                    .map(|s| s.source_solver_lamports),
                "SOL",
                9
            )
        );
        println!(
            "{} {}",
            style("  USDC: ").bold().green(),
            format_balance(
                source_solver_usdc,
                self.balances_snapshot
                    .as_ref()
                    .map(|s| s.source_solver_usdc),
                "USDC",
                6
            )
        );

        println!("{}", style("Destination SVM actors: ").bold().magenta());

        println!("{}", style("Destination user: ").bold().green());
        println!(
            "{} {}",
            style("  Lamports: ").bold().green(),
            format_balance(
                destination_user_lamports,
                self.balances_snapshot
                    .as_ref()
                    .map(|s| s.destination_user_lamports),
                "SOL",
                9
            )
        );
        println!(
            "{} {}",
            style("  USDC: ").bold().green(),
            format_balance(
                destination_user_usdc,
                self.balances_snapshot
                    .as_ref()
                    .map(|s| s.destination_user_usdc),
                "USDC",
                6
            )
        );

        println!("{}", style("Fee payer: ").bold().green());
        println!(
            "{} {}",
            style("  Lamports: ").bold().green(),
            format_balance(
                destination_fee_payer_lamports,
                self.balances_snapshot
                    .as_ref()
                    .map(|s| s.destination_fee_payer_lamports),
                "SOL",
                9
            )
        );

        println!("{}", style("Solver: ").bold().green());
        println!(
            "{} {}",
            style("  Lamports: ").bold().green(),
            format_balance(
                destination_solver_lamports,
                self.balances_snapshot
                    .as_ref()
                    .map(|s| s.destination_solver_lamports),
                "SOL",
                9
            )
        );
        println!(
            "{} {}",
            style("  USDC: ").bold().green(),
            format_balance(
                destination_solver_usdc,
                self.balances_snapshot
                    .as_ref()
                    .map(|s| s.destination_solver_usdc),
                "USDC",
                6
            )
        );

        self.snapshot();

        Ok(())
    }

    pub fn snapshot(&mut self) {
        fn get_token_balance(svm: &LiteSVM, token_address: Pubkey) -> u64 {
            svm.get_account(&token_address)
                .map(|a| {
                    spl_token_2022::state::Account::unpack(&a.data)
                        .map(|a| a.amount)
                        .unwrap_or(0)
                })
                .unwrap_or(0)
        }

        let source_user_lamports =
            helpers::read_account_lamports(self.source_svm, &self.source_user.pubkey())
                .unwrap_or(0);
        let destination_user_lamports =
            helpers::read_account_lamports(self.destination_svm, &self.destination_user.pubkey())
                .unwrap_or(0);
        let source_solver_lamports =
            helpers::read_account_lamports(self.source_svm, &self.solver.pubkey()).unwrap_or(0);
        let destination_solver_lamports =
            helpers::read_account_lamports(self.destination_svm, &self.solver.pubkey())
                .unwrap_or(0);
        let source_fee_payer_lamports =
            helpers::read_account_lamports(self.source_svm, &self.fee_payer.pubkey()).unwrap_or(0);
        let destination_fee_payer_lamports =
            helpers::read_account_lamports(self.destination_svm, &self.fee_payer.pubkey())
                .unwrap_or(0);

        let source_user_usdc = get_token_balance(
            self.source_svm,
            spl_associated_token_account::get_associated_token_address_with_program_id(
                &self.source_user.pubkey(),
                &self.source_usdc_mint.pubkey(),
                &self.token_program_id,
            ),
        );
        let destination_user_usdc = get_token_balance(
            self.destination_svm,
            spl_associated_token_account::get_associated_token_address_with_program_id(
                &self.destination_user.pubkey(),
                &self.destination_usdc_mint.pubkey(),
                &self.token_program_id,
            ),
        );
        let source_solver_usdc = get_token_balance(
            self.source_svm,
            spl_associated_token_account::get_associated_token_address_with_program_id(
                &self.solver.pubkey(),
                &self.source_usdc_mint.pubkey(),
                &self.token_program_id,
            ),
        );
        let destination_solver_usdc = get_token_balance(
            self.destination_svm,
            spl_associated_token_account::get_associated_token_address_with_program_id(
                &self.solver.pubkey(),
                &self.destination_usdc_mint.pubkey(),
                &self.token_program_id,
            ),
        );

        let intent_lamports =
            helpers::read_account_lamports_re(self.source_svm, &Intent::pda(self.intent_hash).0)
                .unwrap_or(0);
        let intent_usdc = get_token_balance(
            self.source_svm,
            Pubkey::find_program_address(
                &[
                    b"reward",
                    self.intent_hash.as_ref(),
                    self.source_usdc_mint.pubkey().as_ref(),
                ],
                &eco_routes::ID,
            )
            .0,
        );

        let balances_snapshot = BalancesSnapshot {
            source_user_lamports,
            source_user_usdc,
            destination_user_lamports,
            destination_user_usdc,
            source_solver_lamports,
            source_solver_usdc,
            destination_solver_lamports,
            destination_solver_usdc,
            source_fee_payer_lamports,
            destination_fee_payer_lamports,
            intent_lamports,
            intent_usdc,
        };

        self.balances_snapshot = Some(balances_snapshot);
    }
}

fn initialize_context<'a>(
    source_svm: &'a mut LiteSVM,
    destination_svm: &'a mut LiteSVM,
    token_program_id: Pubkey,
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

    let usdc_mint_data = &mut [0u8; spl_token_2022::state::Mint::LEN];
    spl_token_2022::state::Mint::pack(
        spl_token_2022::state::Mint {
            decimals: usdc_decimals(),
            is_initialized: true,
            ..spl_token_2022::state::Mint::default()
        },
        usdc_mint_data,
    )?;

    helpers::write_account_re(
        source_svm,
        source_usdc_mint.pubkey(),
        token_program_id,
        usdc_mint_data.to_vec(),
    )?;

    helpers::write_account_re(
        destination_svm,
        destination_usdc_mint.pubkey(),
        token_program_id,
        usdc_mint_data.to_vec(),
    )?;

    // Initialize token accounts

    let source_user_usdc_token_pubkey =
        spl_associated_token_account::get_associated_token_address_with_program_id(
            &source_user.pubkey(),
            &source_usdc_mint.pubkey(),
            &token_program_id,
        );

    let source_user_usdc_token_data = &mut [0u8; spl_token_2022::state::Account::LEN];
    spl_token_2022::state::Account::pack(
        spl_token_2022::state::Account {
            amount: usdc_amount(5.0),
            mint: source_usdc_mint.pubkey(),
            owner: source_user.pubkey(),
            state: spl_token_2022::state::AccountState::Initialized,
            ..spl_token_2022::state::Account::default()
        },
        source_user_usdc_token_data,
    )?;

    helpers::write_account_re(
        source_svm,
        source_user_usdc_token_pubkey,
        token_program_id,
        source_user_usdc_token_data.to_vec(),
    )?;

    let solver_source_usdc_token_pubkey =
        spl_associated_token_account::get_associated_token_address_with_program_id(
            &solver.pubkey(),
            &source_usdc_mint.pubkey(),
            &token_program_id,
        );

    let solver_source_usdc_token_data = &mut [0u8; spl_token_2022::state::Account::LEN];
    spl_token_2022::state::Account::pack(
        spl_token_2022::state::Account {
            amount: usdc_amount(0.0),
            mint: source_usdc_mint.pubkey(),
            owner: solver.pubkey(),
            state: spl_token_2022::state::AccountState::Initialized,
            ..spl_token_2022::state::Account::default()
        },
        solver_source_usdc_token_data,
    )?;

    helpers::write_account_re(
        source_svm,
        solver_source_usdc_token_pubkey,
        token_program_id,
        solver_source_usdc_token_data.to_vec(),
    )?;

    let solver_destination_usdc_token_pubkey =
        spl_associated_token_account::get_associated_token_address_with_program_id(
            &solver.pubkey(),
            &destination_usdc_mint.pubkey(),
            &token_program_id,
        );

    let solver_destination_usdc_token_data = &mut [0u8; spl_token_2022::state::Account::LEN];
    spl_token_2022::state::Account::pack(
        spl_token_2022::state::Account {
            amount: usdc_amount(5.0),
            mint: destination_usdc_mint.pubkey(),
            owner: solver.pubkey(),
            state: spl_token_2022::state::AccountState::Initialized,
            ..spl_token_2022::state::Account::default()
        },
        solver_destination_usdc_token_data,
    )?;

    helpers::write_account_re(
        destination_svm,
        solver_destination_usdc_token_pubkey,
        token_program_id,
        solver_destination_usdc_token_data.to_vec(),
    )?;

    // Intent parameters

    let salt = helpers::generate_salt();

    let execution_authority_pubkey = execution_authority_key(&salt).0;

    let create_ata_instruction =
        spl_associated_token_account::instruction::create_associated_token_account(
            &SOLVER_PLACEHOLDER_PUBKEY,
            &destination_user.pubkey(),
            &destination_usdc_mint.pubkey(),
            &token_program_id,
        );

    let mut transfer_instruction = spl_token_2022::instruction::transfer_checked(
        &token_program_id,
        &spl_associated_token_account::get_associated_token_address_with_program_id(
            &execution_authority_pubkey,
            &destination_usdc_mint.pubkey(),
            &token_program_id,
        ),
        &destination_usdc_mint.pubkey(),
        &spl_associated_token_account::get_associated_token_address_with_program_id(
            &destination_user.pubkey(),
            &destination_usdc_mint.pubkey(),
            &token_program_id,
        ),
        &execution_authority_pubkey,
        &[],
        usdc_amount(5.0),
        usdc_decimals(),
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
        inbox: Pubkey::find_program_address(&[b"dispatch_authority"], &eco_routes::ID)
            .0
            .to_bytes(),
        tokens: vec![TokenAmount {
            token: destination_usdc_mint.pubkey().to_bytes(),
            amount: usdc_amount(5.0),
        }],
        calls: vec![
            Call {
                destination: spl_associated_token_account::ID.to_bytes(),
                calldata: SvmCallDataWithAccountMetas {
                    svm_call_data: SvmCallData {
                        instruction_data: create_ata_instruction.data,
                        num_account_metas: create_ata_instruction.accounts.len() as u8,
                    },
                    account_metas: create_ata_instruction
                        .accounts
                        .clone()
                        .into_iter()
                        .map(SerializableAccountMeta::from)
                        .collect(),
                }
                .try_to_vec()?,
            },
            Call {
                destination: token_program_id.to_bytes(),
                calldata: SvmCallDataWithAccountMetas {
                    svm_call_data: SvmCallData {
                        instruction_data: transfer_instruction.data,
                        num_account_metas: transfer_instruction.accounts.len() as u8,
                    },
                    account_metas: transfer_instruction
                        .accounts
                        .into_iter()
                        .map(SerializableAccountMeta::from)
                        .collect(),
                }
                .try_to_vec()?,
            },
        ],
    };

    let reward = Reward {
        creator: source_user.pubkey(),
        prover: eco_routes::ID.to_bytes(),
        tokens: vec![TokenAmount {
            token: source_usdc_mint.pubkey().to_bytes(),
            amount: usdc_amount(5.0),
        }],
        native_amount: sol_amount(0.03),
        deadline: 100,
    };

    let intent_hash = eco_routes::encoding::intent_hash(&route, &reward);

    Ok(Context {
        token_program_id,
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
        balances_snapshot: None,
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
        status: IntentStatus::Funding(false, 0),
        route: context.route.clone(),
        reward: context.reward.clone(),
        solver: None,
        bump: Intent::pda(context.intent_hash).1,
    };

    assert_eq!(
        intent.route, expected_intent.route,
        "Malformed Intent route state"
    );
    assert_eq!(
        intent.reward, expected_intent.reward,
        "Malformed Intent reward state"
    );

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
                        funder_token: spl_associated_token_account::get_associated_token_address_with_program_id(
                            &context.source_user.pubkey(),
                            &context.source_usdc_mint.pubkey(),
                            &context.token_program_id
                        ),
                        vault: Pubkey::find_program_address(
                            &[
                                b"reward",
                                context.intent_hash.as_ref(),
                                context.source_usdc_mint.pubkey().as_ref(),
                            ],
                            &eco_routes::ID,
                        )
                        .0,
                        mint: context.source_usdc_mint.pubkey(),
                        token_program: context.token_program_id,
                    }
                    .to_account_metas(None),
                    data: fund_intent_spl_args.data(),
                },
            ],
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

    assert_eq!(
        intent.status,
        IntentStatus::Funded,
        "Intent status should be funded"
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
        let call_data_with_account_metas =
            SvmCallDataWithAccountMetas::try_from_slice(&c.calldata).unwrap();
        c.calldata = call_data_with_account_metas
            .svm_call_data
            .try_to_vec()
            .unwrap();
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
                &context.token_program_id,
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
                            pubkey: spl_associated_token_account::get_associated_token_address_with_program_id(
                                &context.solver.pubkey(),
                                &Pubkey::new_from_array(t.token),
                                &context.token_program_id,
                            ),
                            is_signer: false,
                            is_writable: true,
                        },
                        AccountMeta {
                            pubkey: spl_associated_token_account::get_associated_token_address_with_program_id(
                                &execution_authority_key(&context.route.salt).0,
                                &Pubkey::new_from_array(t.token),
                                &context.token_program_id,
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
                SvmCallDataWithAccountMetas::try_from_slice(&c.calldata)
                    .unwrap()
                    .account_metas
                    .into_iter()
                    .map(|m: SerializableAccountMeta| AccountMeta {
                        pubkey: if m.pubkey == SOLVER_PLACEHOLDER_PUBKEY {
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

    let destination_usdc_token =
        spl_associated_token_account::get_associated_token_address_with_program_id(
            &context.destination_user.pubkey(),
            &context.destination_usdc_mint.pubkey(),
            &context.token_program_id,
        );

    let destination_usdc_token_data = context
        .destination_svm
        .get_account(&destination_usdc_token)
        .ok_or_else(|| anyhow::anyhow!("destination usdc token account missing"))?;

    let destination_usdc_token_account = spl_token_2022::extension::StateWithExtensions::<
        spl_token_2022::state::Account,
    >::unpack(&destination_usdc_token_data.data)?
    .base;

    assert_eq!(
        destination_usdc_token_account.amount,
        usdc_amount(5.0),
        "destination usdc token amount should be 5"
    );

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

    fn build_multisig_metadata() -> Vec<u8> {
        const SIG_LEN: usize = 65;
        let mut meta = Vec::with_capacity(32 + 32 + 4 + SIG_LEN);

        meta.extend_from_slice(&MAILBOX_ID.as_ref()); // origin mailbox
        meta.extend_from_slice(&[0u8; 32]); // merkle root = 0
        meta.extend_from_slice(&0u32.to_be_bytes()); // merkle index = 0
        meta.extend_from_slice(&[0u8; 64]); // r + s = 0
        meta.push(27u8); // v = 27 (valid)

        meta
    }

    #[derive(borsh::BorshDeserialize, borsh::BorshSerialize, Debug, PartialEq)]
    pub struct InboxProcess {
        /// The metadata required by the ISM to process the message.
        pub metadata: Vec<u8>,
        /// The encoded message.
        pub message: Vec<u8>,
    }

    let inbox_process = InboxProcess {
        metadata: build_multisig_metadata(),
        message: message_bytes,
    };

    let mut ix_data = vec![1u8]; // enum tag: InboxProcess
    ix_data.extend_from_slice(borsh::to_vec(&inbox_process)?.as_slice());

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
                Pubkey::find_program_address(
                    &[b"test_ism", b"-", b"storage"],
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
                        vault: Pubkey::find_program_address(
                            &[
                                b"reward",
                                context.intent_hash.as_ref(),
                                context.source_usdc_mint.pubkey().as_ref(),
                            ],
                            &eco_routes::ID,
                        )
                        .0,
                        claimer_token: spl_associated_token_account::get_associated_token_address_with_program_id(
                            &context.solver.pubkey(),
                            &context.source_usdc_mint.pubkey(),
                            &context.token_program_id
                        ),
                        mint: context.source_usdc_mint.pubkey(),
                        token_program: context.token_program_id,
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
        IntentStatus::Claimed(true, intent.reward.tokens.len() as u8),
        "Intent status should be claimed"
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
