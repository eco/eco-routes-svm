//! Hand-rolled mirror of the pieces of Polymer's deployed `polymer_prover`
//! program (polymerdao/solana-prover-contracts v1.0.4) that this repository
//! consumes: the program itself (`validate_event`, the result account, the
//! PDAs) plus the relayer-side selectors the real-binary smoke test drives
//! (`create_accounts`, `load_proof`, `InternalAccount`). Kept as constants
//! rather than a crate dependency so Polymer's crypto dependencies and Anchor
//! pin stay out of our build graph.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke;

use crate::instructions::PolymerProverError;

// Both clusters' IDs are named so the PDA golden can pin both derivations
// whatever feature the test build uses. `.github/workflows/polymer-upstream-drift.yml`
// greps these two declarations (whitespace-insensitively, so rustfmt may wrap
// them) to keep its matrix in step; `cluster_ids_are_polymers_deployments`
// pins the literals on the Rust side.
pub const MAINNET_POLYMER_PROVER_ID: Pubkey =
    pubkey!("CdvSq48QUukYuMczgZAVNZrwcHNshBdtqrjW26sQiGPs");
pub const DEVNET_POLYMER_PROVER_ID: Pubkey =
    pubkey!("FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3");

#[cfg(feature = "mainnet")]
pub const POLYMER_PROVER_ID: Pubkey = MAINNET_POLYMER_PROVER_ID;
#[cfg(not(feature = "mainnet"))]
pub const POLYMER_PROVER_ID: Pubkey = DEVNET_POLYMER_PROVER_ID;

pub const CACHE_SEED: &[u8] = b"cache";
pub const RESULT_SEED: &[u8] = b"result";
pub const INTERNAL_SEED: &[u8] = b"internal";

/// `sha256("global:validate_event")[..8]`
pub const VALIDATE_EVENT_DISCRIMINATOR: [u8; 8] = [0x42, 0xf9, 0xcf, 0xdd, 0x1e, 0x57, 0x1b, 0x81];
/// `sha256("global:create_accounts")[..8]`
pub const CREATE_ACCOUNTS_DISCRIMINATOR: [u8; 8] = [0x5e, 0xaf, 0x6d, 0xaa, 0xad, 0x0b, 0x19, 0xb0];
/// `sha256("global:load_proof")[..8]`
pub const LOAD_PROOF_DISCRIMINATOR: [u8; 8] = [0x22, 0x91, 0x55, 0x09, 0x48, 0x62, 0x11, 0x5c];
/// `sha256("account:ValidationResultAccount")[..8]`
pub const VALIDATION_RESULT_DISCRIMINATOR: [u8; 8] =
    [0xa0, 0x95, 0x47, 0x4c, 0x94, 0x30, 0x55, 0xe5];
/// `sha256("account:InternalAccount")[..8]`
pub const INTERNAL_ACCOUNT_DISCRIMINATOR: [u8; 8] =
    [0x97, 0x95, 0xe9, 0x4e, 0x8f, 0x0c, 0x11, 0xfe];

pub fn cache_pda(authority: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[CACHE_SEED, authority.as_ref()], &POLYMER_PROVER_ID)
}

pub fn result_pda(authority: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[RESULT_SEED, authority.as_ref()], &POLYMER_PROVER_ID)
}

pub fn internal_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[INTERNAL_SEED], &POLYMER_PROVER_ID)
}

/// Body of Polymer's `ValidationResultAccount`, field order and types verbatim.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub error_message: String,
    pub chain_id: u32,
    pub emitting_contract: [u8; 20],
    pub topics: Vec<u8>,
    pub unindexed_data: Vec<u8>,
}

impl ValidationResult {
    /// Decodes the account Polymer wrote in `validate_event`. Requires the
    /// account to be owned by Polymer's program and to carry its discriminator.
    pub fn try_from_account_info(account: &AccountInfo<'_>) -> Result<Self> {
        require!(
            *account.owner == POLYMER_PROVER_ID,
            PolymerProverError::InvalidResultAccount
        );
        let data = account.try_borrow_data()?;
        let (discriminator, body) = data
            .split_at_checked(8)
            .ok_or(PolymerProverError::InvalidResultAccount)?;
        require!(
            discriminator == VALIDATION_RESULT_DISCRIMINATOR,
            PolymerProverError::InvalidResultAccount
        );

        Self::from_body(body)
    }

    /// Borsh-decodes a body that may be followed by zero padding: Polymer
    /// allocates the account at `INIT_SPACE`, so `try_from_slice` (which
    /// insists on consuming every byte) would reject a valid account.
    pub fn from_body(mut body: &[u8]) -> Result<Self> {
        AnchorDeserialize::deserialize(&mut body)
            .map_err(|_| PolymerProverError::InvalidResultAccount.into())
    }
}

/// CPIs Polymer's `validate_event`. `authority` must be a signer of the outer
/// transaction; its signature passes through a plain `invoke`.
pub fn validate_event<'info>(
    polymer_prover_program: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    cache_account: &AccountInfo<'info>,
    result_account: &AccountInfo<'info>,
    internal: &AccountInfo<'info>,
) -> Result<()> {
    let ix = Instruction {
        program_id: polymer_prover_program.key(),
        accounts: vec![
            AccountMeta::new(authority.key(), true),
            AccountMeta::new(cache_account.key(), false),
            AccountMeta::new(result_account.key(), false),
            AccountMeta::new_readonly(internal.key(), false),
        ],
        data: VALIDATE_EVENT_DISCRIMINATOR.to_vec(),
    };

    invoke(
        &ix,
        &[
            authority.to_account_info(),
            cache_account.to_account_info(),
            result_account.to_account_info(),
            internal.to_account_info(),
        ],
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use solana_sha256_hasher::hash;

    use super::*;

    fn anchor_discriminator(name: &str) -> [u8; 8] {
        hash(name.as_bytes()).to_bytes()[..8].try_into().unwrap()
    }

    #[test]
    fn instruction_discriminators_match_anchor_derivation() {
        assert_eq!(
            VALIDATE_EVENT_DISCRIMINATOR,
            anchor_discriminator("global:validate_event")
        );
        assert_eq!(
            CREATE_ACCOUNTS_DISCRIMINATOR,
            anchor_discriminator("global:create_accounts")
        );
        assert_eq!(
            LOAD_PROOF_DISCRIMINATOR,
            anchor_discriminator("global:load_proof")
        );
    }

    #[test]
    fn account_discriminators_match_anchor_derivation() {
        assert_eq!(
            VALIDATION_RESULT_DISCRIMINATOR,
            anchor_discriminator("account:ValidationResultAccount")
        );
        assert_eq!(
            INTERNAL_ACCOUNT_DISCRIMINATOR,
            anchor_discriminator("account:InternalAccount")
        );
    }

    /// Pins the PDA derivations under both clusters' Polymer program IDs
    /// explicitly, not under the cfg-selected one, so the golden is identical
    /// with and without `--features mainnet` and runs in both profiles.
    #[test]
    fn pdas_deterministic() {
        let authority = Pubkey::new_from_array([7u8; 32]);
        let pdas = |program: &Pubkey| {
            (
                Pubkey::find_program_address(&[CACHE_SEED, authority.as_ref()], program),
                Pubkey::find_program_address(&[RESULT_SEED, authority.as_ref()], program),
                Pubkey::find_program_address(&[INTERNAL_SEED], program),
            )
        };
        goldie::assert_json!((
            pdas(&DEVNET_POLYMER_PROVER_ID),
            pdas(&MAINNET_POLYMER_PROVER_ID)
        ));
    }

    /// Feature-independent twin of the drift workflow's matrix grep: the two
    /// literals are Polymer's deployments (polymerdao/solana-prover-contracts
    /// v1.0.4), so a refactor of the declarations cannot swap or retype them.
    #[test]
    fn cluster_ids_are_polymers_deployments() {
        assert_eq!(
            MAINNET_POLYMER_PROVER_ID,
            pubkey!("CdvSq48QUukYuMczgZAVNZrwcHNshBdtqrjW26sQiGPs")
        );
        assert_eq!(
            DEVNET_POLYMER_PROVER_ID,
            pubkey!("FtdxWoZXZKNYn1Dx9XXDE5hKXWf69tjFJUofNZuaWUH3")
        );
    }

    #[test]
    fn polymer_prover_id_tracks_the_mainnet_feature() {
        #[cfg(feature = "mainnet")]
        assert_eq!(POLYMER_PROVER_ID, MAINNET_POLYMER_PROVER_ID);
        #[cfg(not(feature = "mainnet"))]
        assert_eq!(POLYMER_PROVER_ID, DEVNET_POLYMER_PROVER_ID);
    }

    /// The helpers derive under the cfg-selected ID; `pdas_deterministic` pins
    /// the bytes, this pins the wiring between the two.
    #[test]
    fn pda_helpers_derive_under_the_selected_id() {
        let authority = Pubkey::new_from_array([7u8; 32]);
        assert_eq!(
            cache_pda(&authority),
            Pubkey::find_program_address(&[CACHE_SEED, authority.as_ref()], &POLYMER_PROVER_ID)
        );
        assert_eq!(
            result_pda(&authority),
            Pubkey::find_program_address(&[RESULT_SEED, authority.as_ref()], &POLYMER_PROVER_ID)
        );
        assert_eq!(
            internal_pda(),
            Pubkey::find_program_address(&[INTERNAL_SEED], &POLYMER_PROVER_ID)
        );
    }

    fn sample_result() -> ValidationResult {
        ValidationResult {
            is_valid: true,
            error_message: String::new(),
            chain_id: 8453,
            emitting_contract: [0xab; 20],
            topics: vec![1u8; 64],
            unindexed_data: vec![2u8; 96],
        }
    }

    /// `discriminator ‖ borsh(body) ‖ zero padding`, the shape of the account
    /// Polymer allocates at `INIT_SPACE` and writes in `validate_event`.
    fn account_data(discriminator: [u8; 8], body: &ValidationResult) -> Vec<u8> {
        let mut data = discriminator.to_vec();
        data.extend(borsh::to_vec(body).unwrap());
        data.extend(std::iter::repeat_n(0u8, 512));
        data
    }

    #[test]
    fn validation_result_roundtrip_ignores_trailing_padding() {
        let expected = sample_result();
        let mut body = borsh::to_vec(&expected).unwrap();
        // Polymer allocates the account at max size, so the serialized body is
        // followed by zero padding that a strict decoder would reject.
        body.extend(std::iter::repeat_n(0u8, 512));

        let decoded = ValidationResult::from_body(&body).unwrap();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn validation_result_rejects_truncated_body() {
        let body = vec![1u8; 5];
        assert!(ValidationResult::from_body(&body).is_err());
    }

    /// The positive arm that keeps the three rejections below from passing
    /// vacuously: a Polymer-owned account with the right discriminator decodes.
    #[test]
    fn try_from_account_info_decodes_polymer_owned_account() {
        let expected = sample_result();
        let mut data = account_data(VALIDATION_RESULT_DISCRIMINATOR, &expected);
        let key = Pubkey::new_unique();
        let owner = POLYMER_PROVER_ID;
        let mut lamports = 0u64;
        let account = AccountInfo::new(&key, false, false, &mut lamports, &mut data, &owner, false);

        assert_eq!(
            ValidationResult::try_from_account_info(&account).unwrap(),
            expected
        );
    }

    /// The owner check is what makes the result authentic: the same bytes under
    /// any other program are a look-alike, not Polymer's verdict. Unreachable
    /// through a transaction (the `address` constraint pins the slot to Polymer's
    /// PDA), which is why it is pinned here.
    #[test]
    fn try_from_account_info_rejects_account_owned_by_another_program() {
        let mut data = account_data(VALIDATION_RESULT_DISCRIMINATOR, &sample_result());
        let key = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let mut lamports = 0u64;
        let account = AccountInfo::new(&key, false, false, &mut lamports, &mut data, &owner, false);

        assert_eq!(
            ValidationResult::try_from_account_info(&account).unwrap_err(),
            PolymerProverError::InvalidResultAccount.into()
        );
    }

    #[test]
    fn try_from_account_info_rejects_wrong_discriminator() {
        let mut discriminator = VALIDATION_RESULT_DISCRIMINATOR;
        discriminator[0] ^= 0xff;
        let mut data = account_data(discriminator, &sample_result());
        let key = Pubkey::new_unique();
        let owner = POLYMER_PROVER_ID;
        let mut lamports = 0u64;
        let account = AccountInfo::new(&key, false, false, &mut lamports, &mut data, &owner, false);

        assert_eq!(
            ValidationResult::try_from_account_info(&account).unwrap_err(),
            PolymerProverError::InvalidResultAccount.into()
        );
    }

    /// Models an account shorter than the discriminator (the `split_at_checked`
    /// exit), e.g. one that was never initialised.
    #[test]
    fn try_from_account_info_rejects_data_shorter_than_discriminator() {
        let mut data = VALIDATION_RESULT_DISCRIMINATOR[..4].to_vec();
        let key = Pubkey::new_unique();
        let owner = POLYMER_PROVER_ID;
        let mut lamports = 0u64;
        let account = AccountInfo::new(&key, false, false, &mut lamports, &mut data, &owner, false);

        assert_eq!(
            ValidationResult::try_from_account_info(&account).unwrap_err(),
            PolymerProverError::InvalidResultAccount.into()
        );
    }
}
