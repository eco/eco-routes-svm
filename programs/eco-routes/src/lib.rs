use anchor_lang::prelude::*;

declare_id!("3zbEiMYyf4y1bGsVBAzKrXVzMndRQdTMDgx3aKCs8BHs");

pub mod error;
pub mod instructions;
pub mod state;

#[program]
pub mod eco_routes {
    use super::*;
}
