use anchor_lang::prelude::*;

pub const DISPATCHER_SEED: &[u8] = b"dispatcher";

pub fn dispatcher_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[DISPATCHER_SEED], &crate::ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatcher_pda_deterministic() {
        goldie::assert_json!(dispatcher_pda());
    }
}
