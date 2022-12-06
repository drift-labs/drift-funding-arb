use anchor_client::anchor_lang::AccountDeserialize;
use anchor_client::solana_client::rpc_client::RpcClient;
use anchor_client::solana_sdk::pubkey::Pubkey;

use drift::state::user::User;
use drift::state::state::State;
use drift::state::perp_market::PerpMarket;
use drift::state::spot_market::{SpotMarket};

use crate::constants::*;

pub fn get_perp_market_public_key(market_index: u16, program_id: &Pubkey) -> Pubkey { 
    Pubkey::find_program_address(&[b"perp_market", market_index.to_le_bytes().as_ref()], program_id).0
}

pub fn get_spot_market_public_key(market_index: u16, program_id: &Pubkey) -> Pubkey { 
    Pubkey::find_program_address(&[b"spot_market", market_index.to_le_bytes().as_ref()], program_id).0
}

pub fn get_user_public_key(owner: &Pubkey, subaccount_id: u16, program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"user", owner.as_ref(), subaccount_id.to_le_bytes().as_ref()], 
        program_id
    ).0
}

pub fn get_user_stats_public_key(owner: &Pubkey, program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"user_stats", owner.as_ref()], 
        program_id
    ).0
}

pub fn get_state_public_key(program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"drift_state"], 
        program_id
    ).0
}

pub fn get_spot_market_vault_public_key(market_index: u16, program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"spot_market_vault", market_index.to_le_bytes().as_ref()], 
        program_id
    ).0
}

pub fn get_drift_signer_public_key(program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"drift_signer"], 
        program_id
    ).0
}

pub fn get_user(connection: &RpcClient, address: &Pubkey) -> Result<User> {
    let data = &mut &*connection.get_account_data(address)?;
    let user = User::try_deserialize(data)?;
    Ok(user)
}

pub fn get_perp_market(connection: &RpcClient, address: &Pubkey) -> Result<PerpMarket> {
    let data = &mut &*connection.get_account_data(address)?;
    let perp_market = PerpMarket::try_deserialize(data)?;
    Ok(perp_market)
}

pub fn get_spot_market(connection: &RpcClient, address: &Pubkey) -> Result<SpotMarket> {
    let data = &mut &*connection.get_account_data(address)?;
    let spot_market = SpotMarket::try_deserialize(data)?;
    Ok(spot_market)
}

pub fn get_state(connection: &RpcClient, address: &Pubkey) -> Result<State> {
    let data = &mut &*connection.get_account_data(address)?;
    let state = State::try_deserialize(data)?;
    Ok(state)
}

pub fn derive_token_address(
    owner: &Pubkey, 
    mint: &Pubkey, 
) -> Pubkey {
    let (pda, _) = Pubkey::find_program_address(
        &[
            &owner.to_bytes(),
            &TOKEN_PROGRAM_ID.to_bytes(),
            &mint.to_bytes(),
        ],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    );
    pda
}