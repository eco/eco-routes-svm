use anchor_lang::{InstructionData, ToAccountMetas};
use eco_svm_std::Bytes32;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;

use crate::chain::{self, Chain};
use crate::config::{self, Configs};
use crate::plan::{
    self, Plan, AGGREGATOR_MEMBERS, AGGREGATOR_PROVER, HYPER_PROVER, LAYERZERO_PROVER,
    POLYMER_PROVER,
};
use crate::{funding, layerzero};

const FUND_RESERVE: &str = "fund_reserve";
const INIT: &str = "init";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Chain(#[from] chain::Error),
    #[error(transparent)]
    Config(#[from] config::Error),
    #[error("{program}: on-chain config is {actual}, expected {expected}")]
    ConfigMismatch {
        program: &'static str,
        expected: String,
        actual: String,
    },
    #[error(transparent)]
    LayerZero(#[from] layerzero::Error),
    #[error(transparent)]
    Plan(#[from] plan::Error),
    #[error("aggregator members are not deployed programs: {members:?}")]
    MembersNotDeployed { members: Vec<String> },
    #[error("{program} is not in the plan")]
    UnplannedProgram { program: &'static str },
}

#[derive(Debug, Clone)]
pub struct Step {
    pub program: &'static str,
    pub action: &'static str,
    /// `None` when the step was already done and nothing was sent.
    pub signature: Option<Signature>,
}

type StepFn<C> = fn(&mut C, &Plan, &Keypair) -> Result<Step, Error>;

/// Runs the init steps in spec order, calling `observe` with each step as soon as it has landed
/// so a failure part-way still shows what was written. Configs are re-verified before the first
/// write, and each step reads before it writes, so a re-run sends nothing.
pub fn apply<C: Chain>(
    chain: &mut C,
    plan: &Plan,
    deployer: &Keypair,
    observe: &mut impl FnMut(&Step),
) -> Result<(), Error> {
    plan.verify_configs(&config::read(chain, &plan.release)?)?;
    let prover_steps: [StepFn<C>; 5] = [
        fund_hyper_reserve,
        fund_layerzero_reserve,
        init_hyper,
        init_polymer,
        init_aggregator,
    ];

    prover_steps.into_iter().try_for_each(|run| {
        observe(&run(chain, plan, deployer)?);

        Ok::<_, Error>(())
    })?;

    Ok(layerzero::apply(chain, plan, deployer, &mut |action| {
        observe(&step(LAYERZERO_PROVER, action.name, action.signature))
    })?)
}

fn fund_hyper_reserve(
    chain: &mut impl Chain,
    plan: &Plan,
    deployer: &Keypair,
) -> Result<Step, Error> {
    let seed = hyper_prover::state::PDA_PAYER_SEED;

    fund_reserve(
        chain,
        deployer,
        HYPER_PROVER,
        &plan_address(plan, HYPER_PROVER)?,
        seed,
        plan.inputs.hyper_reserve_lamports,
    )
}

fn fund_layerzero_reserve(
    chain: &mut impl Chain,
    plan: &Plan,
    deployer: &Keypair,
) -> Result<Step, Error> {
    let seed = layerzero_prover::state::PDA_PAYER_SEED;

    fund_reserve(
        chain,
        deployer,
        LAYERZERO_PROVER,
        &plan_address(plan, LAYERZERO_PROVER)?,
        seed,
        plan.inputs.layerzero_reserve_lamports,
    )
}

fn fund_reserve(
    chain: &mut impl Chain,
    deployer: &Keypair,
    program: &'static str,
    address: &Pubkey,
    seed: &[u8],
    requested_lamports: u64,
) -> Result<Step, Error> {
    let reserve = Pubkey::find_program_address(&[seed], address).0;
    let top_up = funding::reserve_top_up(chain, &reserve, requested_lamports)?;
    let signature = match top_up {
        0 => None,
        top_up => {
            let transfer = solana_system_interface::instruction::transfer(
                &deployer.pubkey(),
                &reserve,
                top_up,
            );

            Some(chain.send(&[transfer], &[deployer])?)
        }
    };

    Ok(step(program, FUND_RESERVE, signature))
}

fn init_hyper(chain: &mut impl Chain, plan: &Plan, deployer: &Keypair) -> Result<Step, Error> {
    let address = plan_address(plan, HYPER_PROVER)?;
    let live = Configs {
        hyper_senders: config::hyper_senders(chain, &address)?,
        ..Configs::default()
    };
    let whitelisted_senders = plan.inputs.hyper_senders.iter().map(Into::into).collect();
    let instruction = hyper_init_instruction(&address, &deployer.pubkey(), whitelisted_senders);

    init(chain, plan, deployer, HYPER_PROVER, &live, |_| {
        Ok(instruction)
    })
}

fn init_polymer(chain: &mut impl Chain, plan: &Plan, deployer: &Keypair) -> Result<Step, Error> {
    let address = plan_address(plan, POLYMER_PROVER)?;
    let live = Configs {
        polymer_emitters: config::polymer_emitters(chain, &address)?,
        ..Configs::default()
    };
    let whitelisted_emitters = plan
        .inputs
        .polymer_emitters
        .iter()
        .map(Into::into)
        .collect();
    let instruction = polymer_init_instruction(&address, &deployer.pubkey(), whitelisted_emitters);

    init(chain, plan, deployer, POLYMER_PROVER, &live, |_| {
        Ok(instruction)
    })
}

fn init_aggregator<C: Chain>(
    chain: &mut C,
    plan: &Plan,
    deployer: &Keypair,
) -> Result<Step, Error> {
    let address = plan_address(plan, AGGREGATOR_PROVER)?;
    let live = Configs {
        aggregator_provers: config::aggregator_provers(chain, &address)?,
        ..Configs::default()
    };
    let members = AGGREGATOR_MEMBERS
        .into_iter()
        .map(|member| Ok((member, plan_address(plan, member)?)))
        .collect::<Result<Vec<_>, Error>>()?;

    init(
        chain,
        plan,
        deployer,
        AGGREGATOR_PROVER,
        &live,
        |chain: &C| {
            require_deployed(chain, &members)?;

            Ok(aggregator_init_instruction(
                &address,
                &deployer.pubkey(),
                &members,
            ))
        },
    )
}

fn init<C: Chain>(
    chain: &mut C,
    plan: &Plan,
    deployer: &Keypair,
    program: &'static str,
    live: &Configs,
    instruction: impl FnOnce(&C) -> Result<Instruction, Error>,
) -> Result<Step, Error> {
    live.conflict(&plan.expected_configs)?;
    let signature = match live.is_absent() {
        false => None,
        true => Some(chain.send(&[instruction(chain)?], &[deployer])?),
    };

    Ok(step(program, INIT, signature))
}

fn require_deployed(chain: &impl Chain, members: &[(&'static str, Pubkey)]) -> Result<(), Error> {
    let missing = members
        .iter()
        .filter_map(|(name, address)| match chain.account(address) {
            Ok(Some(account)) if account.executable => None,
            Ok(_) => Some(Ok((*name).to_owned())),
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()?;

    match missing.is_empty() {
        true => Ok(()),
        false => Err(Error::MembersNotDeployed { members: missing }),
    }
}

fn hyper_init_instruction(
    program: &Pubkey,
    deployer: &Pubkey,
    whitelisted_senders: Vec<Bytes32>,
) -> Instruction {
    let accounts = hyper_prover::accounts::Init {
        config: config::config_address(hyper_prover::state::CONFIG_SEED, program),
        payer: *deployer,
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        system_program: solana_sdk_ids::system_program::id(),
    };
    let args = hyper_prover::instructions::InitArgs {
        whitelisted_senders,
    };

    Instruction {
        program_id: *program,
        accounts: accounts.to_account_metas(None),
        data: hyper_prover::instruction::Init { args }.data(),
    }
}

fn polymer_init_instruction(
    program: &Pubkey,
    deployer: &Pubkey,
    whitelisted_emitters: Vec<Bytes32>,
) -> Instruction {
    let accounts = polymer_prover::accounts::Init {
        config: config::config_address(polymer_prover::state::CONFIG_SEED, program),
        payer: *deployer,
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        system_program: solana_sdk_ids::system_program::id(),
    };
    let args = polymer_prover::instructions::InitArgs {
        whitelisted_emitters,
    };

    Instruction {
        program_id: *program,
        accounts: accounts.to_account_metas(None),
        data: polymer_prover::instruction::Init { args }.data(),
    }
}

fn aggregator_init_instruction(
    program: &Pubkey,
    deployer: &Pubkey,
    members: &[(&'static str, Pubkey)],
) -> Instruction {
    let accounts = aggregator_prover::accounts::Init {
        payer: *deployer,
        authority: *deployer,
        program: *program,
        program_data: config::program_data_address(program),
        config: config::config_address(aggregator_prover::state::CONFIG_SEED, program),
        system_program: solana_sdk_ids::system_program::id(),
    };
    let member_accounts = members
        .iter()
        .map(|(_, member)| AccountMeta::new_readonly(*member, false));

    Instruction {
        program_id: *program,
        accounts: accounts
            .to_account_metas(None)
            .into_iter()
            .chain(member_accounts)
            .collect(),
        data: aggregator_prover::instruction::Init {}.data(),
    }
}

fn plan_address(plan: &Plan, program: &'static str) -> Result<Pubkey, Error> {
    plan.programs
        .get(program)
        .map(|planned| planned.state.address)
        .ok_or(Error::UnplannedProgram { program })
}

fn step(program: &'static str, action: &'static str, signature: Option<Signature>) -> Step {
    Step {
        program,
        action,
        signature,
    }
}

impl From<config::Mismatch> for Error {
    fn from(mismatch: config::Mismatch) -> Self {
        let config::Mismatch {
            program,
            expected,
            actual,
        } = mismatch;

        Self::ConfigMismatch {
            program,
            expected,
            actual,
        }
    }
}

#[cfg(test)]
mod tests {
    use layerzero_prover::instructions::required_alt_addresses;
    use solana_compute_budget_interface::ComputeBudgetInstruction;
    use solana_sdk::rent::Rent;

    use super::*;
    use crate::classify::Status;
    use crate::testing::{
        chain_with_deployed_members, chain_with_layerzero_store, keys, layerzero_store, pda, plan,
        program_data, release_address,
    };

    const SYSTEM_TRANSFER_DATA_LEN: usize = 12;

    fn init_instruction_keys(
        program: &Pubkey,
        payer: &Pubkey,
        store: &Pubkey,
        lz_receive_types: &Pubkey,
        oapp_registry: &Pubkey,
    ) -> Vec<Pubkey> {
        vec![
            *payer,
            *payer,
            *program,
            program_data(LAYERZERO_PROVER),
            *store,
            *lz_receive_types,
            solana_sdk_ids::system_program::id(),
            layerzero_prover::layerzero::ENDPOINT_ID,
            *oapp_registry,
            layerzero_prover::layerzero::endpoint_event_authority().0,
        ]
    }

    /// Bincode `ExtendLookupTable`: u32 variant tag, u64 length, then the addresses.
    fn extended_addresses(instruction: &Instruction) -> Vec<Pubkey> {
        instruction.data[12..]
            .chunks_exact(32)
            .map(|address| Pubkey::try_from(address).unwrap())
            .collect()
    }

    fn applied(
        chain: &mut impl Chain,
        plan: &Plan,
        deployer: &Keypair,
    ) -> Result<Vec<Step>, Error> {
        let mut steps = Vec::new();
        apply(chain, plan, deployer, &mut |step| steps.push(step.clone()))?;

        Ok(steps)
    }

    fn transfer_lamports(instruction: &Instruction) -> u64 {
        let data = &instruction.data;
        assert_eq!(data.len(), SYSTEM_TRANSFER_DATA_LEN);

        u64::from_le_bytes(data[4..].try_into().unwrap())
    }

    #[test]
    fn instructions_target_release_addresses_not_compiled_ids() {
        let plan = plan(5_000_000);
        let deployer = Keypair::new();
        let payer = deployer.pubkey();
        let system = solana_sdk_ids::system_program::id();
        let mut chain = chain_with_layerzero_store(&plan);

        applied(&mut chain, &plan, &deployer).unwrap();

        let hyper = release_address(HYPER_PROVER);
        let polymer = release_address(POLYMER_PROVER);
        let aggregator = release_address(AGGREGATOR_PROVER);
        let hyper_reserve = pda(hyper_prover::state::PDA_PAYER_SEED, HYPER_PROVER);
        let layerzero_reserve = pda(layerzero_prover::state::PDA_PAYER_SEED, LAYERZERO_PROVER);
        assert_eq!(chain.sent.len(), 13);
        assert_eq!(keys(&chain.sent[0]), vec![payer, hyper_reserve]);
        assert_eq!(keys(&chain.sent[1]), vec![payer, layerzero_reserve]);
        assert_eq!(chain.sent[2].program_id, hyper);
        assert_eq!(
            keys(&chain.sent[2]),
            vec![
                pda(hyper_prover::state::CONFIG_SEED, HYPER_PROVER),
                payer,
                payer,
                hyper,
                program_data(HYPER_PROVER),
                system
            ]
        );
        assert_eq!(chain.sent[3].program_id, polymer);
        assert_eq!(
            keys(&chain.sent[3]),
            vec![
                pda(polymer_prover::state::CONFIG_SEED, POLYMER_PROVER),
                payer,
                payer,
                polymer,
                program_data(POLYMER_PROVER),
                system
            ]
        );
        assert_eq!(chain.sent[4].program_id, aggregator);
        assert_eq!(
            keys(&chain.sent[4]),
            vec![
                payer,
                payer,
                aggregator,
                program_data(AGGREGATOR_PROVER),
                pda(aggregator_prover::state::CONFIG_SEED, AGGREGATOR_PROVER),
                system,
                release_address(HYPER_PROVER),
                release_address(POLYMER_PROVER),
                release_address(LAYERZERO_PROVER),
            ]
        );
    }

    #[test]
    fn layerzero_instructions_target_the_release_address_with_a_compute_unit_limit() {
        let plan = plan(5_000_000);
        let deployer = Keypair::new();
        let payer = deployer.pubkey();
        let system = solana_sdk_ids::system_program::id();
        let program = release_address(LAYERZERO_PROVER);
        let store = pda(layerzero_prover::state::STORE_SEED, LAYERZERO_PROVER);
        let pda_payer = pda(layerzero_prover::state::PDA_PAYER_SEED, LAYERZERO_PROVER);
        let oapp_registry = layerzero_prover::layerzero::oapp_registry_pda(&store).0;
        let mut chain = chain_with_layerzero_store(&plan);

        applied(&mut chain, &plan, &deployer).unwrap();

        let [init_path_limit, init_path, set_path_config_limit, set_path_config, create, extend, freeze, set_alt] =
            &chain.sent[5..]
        else {
            panic!("expected eight LayerZero instructions after the prover inits");
        };
        let limit = ComputeBudgetInstruction::set_compute_unit_limit(1_400_000);
        assert_eq!([init_path_limit, set_path_config_limit], [&limit, &limit]);
        assert_eq!(init_path.program_id, program);
        assert_eq!(
            keys(init_path)[..5],
            [
                payer,
                program,
                program_data(LAYERZERO_PROVER),
                store,
                pda_payer
            ]
        );
        assert_eq!(
            keys(init_path)[5..8],
            [
                system,
                layerzero_prover::layerzero::ENDPOINT_ID,
                oapp_registry
            ]
        );
        assert_eq!(set_path_config.program_id, program);
        assert_eq!(
            keys(set_path_config)[..5],
            [
                payer,
                program,
                program_data(LAYERZERO_PROVER),
                store,
                pda_payer
            ]
        );
        assert_eq!(
            keys(set_path_config)[5..8],
            [
                system,
                layerzero_prover::layerzero::ENDPOINT_ID,
                oapp_registry
            ]
        );
        let alt = keys(create)[0];
        assert_eq!(keys(extend)[0], alt);
        assert_eq!(keys(freeze)[0], alt);
        assert_eq!(set_alt.program_id, program);
        assert_eq!(
            keys(set_alt),
            vec![payer, program, program_data(LAYERZERO_PROVER), store, alt]
        );
        let stored = layerzero_store(&plan);
        assert_eq!(extended_addresses(extend), required_alt_addresses(&stored));
    }

    #[test]
    fn layerzero_init_targets_release_address_when_the_store_is_absent() {
        let plan = plan(5_000_000);
        let deployer = Keypair::new();
        let payer = deployer.pubkey();
        let program = release_address(LAYERZERO_PROVER);
        let store = pda(layerzero_prover::state::STORE_SEED, LAYERZERO_PROVER);
        let oapp_registry = layerzero_prover::layerzero::oapp_registry_pda(&store).0;
        let lz_receive_types = Pubkey::find_program_address(
            &[
                layerzero_prover::layerzero::LZ_RECEIVE_TYPES_SEED,
                store.as_ref(),
            ],
            &program,
        )
        .0;
        let mut chain = chain_with_deployed_members();

        let result = applied(&mut chain, &plan, &deployer);

        assert!(matches!(
            result,
            Err(Error::LayerZero(layerzero::Error::StoreMissing { address })) if address == store
        ));
        let init = &chain.sent[6];
        assert_eq!(init.program_id, program);
        assert_eq!(
            keys(init),
            init_instruction_keys(&program, &payer, &store, &lz_receive_types, &oapp_registry)
        );
    }

    #[test]
    fn steps_are_observed_as_they_land_even_when_a_later_one_fails() {
        let plan = plan(5_000_000);
        let mut chain = chain_with_deployed_members();
        let mut observed = Vec::new();

        let result = apply(&mut chain, &plan, &Keypair::new(), &mut |step| {
            observed.push((step.program, step.action, step.signature.is_some()));
        });

        assert!(result.is_err());
        assert_eq!(
            observed,
            [
                (HYPER_PROVER, FUND_RESERVE, true),
                (LAYERZERO_PROVER, FUND_RESERVE, true),
                (HYPER_PROVER, INIT, true),
                (POLYMER_PROVER, INIT, true),
                (AGGREGATOR_PROVER, INIT, true),
                (LAYERZERO_PROVER, INIT, true),
            ]
        );
    }

    #[test]
    fn reserve_below_rent_exempt_minimum_is_raised_to_it() {
        let plan = plan(1_000);
        let mut chain = chain_with_layerzero_store(&plan);

        applied(&mut chain, &plan, &Keypair::new()).unwrap();

        assert_eq!(
            transfer_lamports(&chain.sent[0]),
            Rent::default().minimum_balance(0)
        );
        assert_eq!(
            transfer_lamports(&chain.sent[1]),
            Rent::default().minimum_balance(0)
        );
    }

    #[test]
    fn zero_reserve_request_sends_no_transfer() {
        let plan = plan(0);
        let mut chain = chain_with_layerzero_store(&plan);

        let report = applied(&mut chain, &plan, &Keypair::new()).unwrap();

        assert!(report[..2].iter().all(|step| step.signature.is_none()));
        assert_eq!(chain.sent.len(), 11);
    }

    #[test]
    fn live_program_without_config_sends_nothing() {
        let mut plan = plan(5_000_000);
        plan.programs.get_mut(HYPER_PROVER).unwrap().state.status = Status::Live;
        let mut chain = chain_with_deployed_members();

        let result = applied(&mut chain, &plan, &Keypair::new());

        assert!(matches!(
            result,
            Err(Error::Plan(plan::Error::LiveWithoutConfig { program })) if program == HYPER_PROVER
        ));
        assert!(chain.sent.is_empty());
    }
}
