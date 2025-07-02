use super::handle_account_metas::SerializableAccountMeta;
use crate::hyperlane::SimulationReturnData;
use anchor_lang::prelude::*;

#[derive(Accounts)]
pub struct IsmAccountMetas {}

pub fn ism_account_metas(
    _ctx: Context<IsmAccountMetas>,
) -> Result<SimulationReturnData<Vec<SerializableAccountMeta>>> {
    Ok(SimulationReturnData::new(vec![]))
}
