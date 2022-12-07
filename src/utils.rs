use std::collections::HashMap;
use std::ops::Mul;
use std::rc::Rc;

use solana_program::instruction::AccountMeta;
use anchor_client::solana_client::rpc_client::RpcClient;
use anchor_client::solana_sdk::commitment_config::CommitmentConfig;
use anchor_client::solana_sdk::pubkey::Pubkey;
use anchor_client::solana_sdk::signature::Signer;
use anchor_client::solana_sdk::signature::read_keypair_file;
use anchor_client::{Client, Cluster};

use drift::state::state::State;
use drift::state::perp_market::PerpMarket;
use drift::state::user::{OrderType, MarketType};
use drift::state::spot_market::SpotMarket;
use drift::state::spot_market::SpotBalanceType;

use drift::math::constants::*;
use drift::math::orders::standardize_base_asset_amount_ceil;

use drift::instructions::OrderParams;
use drift::controller::position::PositionDirection;

// anchor program ixs
use drift::instruction as ix;
use drift::accounts;

use crate::constants::*;
use crate::address::*;

pub enum Market { 
    PerpMarket(PerpMarket), 
    SpotMarket(SpotMarket)
}

macro_rules! cast {
    ($target: expr, $pat: path) => {
        {
            if let $pat(a) = $target { // #1
                a
            } else {
                panic!(
                    "mismatch variant when cast to {}", 
                    stringify!($pat)); // #2
            }
        }
    };
}

pub fn get_cached_accounts(connection: &RpcClient, state_account: &State) -> Result<HashMap<Pubkey, Market>> { 
    let mut cached_accounts: HashMap<Pubkey, Market> = HashMap::new();
    for i in 0..state_account.number_of_markets { 
        let market_pk = get_perp_market_public_key(i, &PROGRAM_ID);
        let market = get_perp_market(connection, &market_pk)?;
        cached_accounts.insert(market_pk, Market::PerpMarket(market));
    }

    for i in 0..state_account.number_of_spot_markets { 
        let spot_pk = get_spot_market_public_key(i, &PROGRAM_ID);
        let spot_market = get_spot_market(connection, &spot_pk)?;
        cached_accounts.insert(spot_pk, Market::SpotMarket(spot_market));
    }
    Ok(cached_accounts)
}

pub fn get_remaining_accounts(state_account: &State, cached_accounts: &HashMap<Pubkey, Market>) -> Result<Vec<AccountMeta>> { 
    let mut perp_market_dict = HashMap::new();
    let mut spot_market_dict = HashMap::new();
    let mut oracle_dict = HashMap::new();

    for i in 0..state_account.number_of_markets { 
        let market_pk = get_perp_market_public_key(i, &PROGRAM_ID);

        let market_meta = AccountMeta {
            pubkey: market_pk, 
            is_signer: false, 
            is_writable: true
        };
        perp_market_dict.insert(market_pk, market_meta);

        let market = cast!(cached_accounts.get(&market_pk).unwrap(), Market::PerpMarket);
        let oracle_meta = AccountMeta { 
            pubkey: market.amm.oracle, 
            is_signer: false, 
            is_writable: false,
        };
        oracle_dict.insert(market.amm.oracle, oracle_meta);
    }

    for i in 0..state_account.number_of_spot_markets { 
        let spot_pk = get_spot_market_public_key(i, &PROGRAM_ID);
        let spot_meta = AccountMeta { 
            pubkey: spot_pk, 
            is_signer: false, 
            is_writable: true, 
        };
        spot_market_dict.insert(spot_pk, spot_meta);

        if i != 0 {
            let spot_market = cast!(cached_accounts.get(&spot_pk).unwrap(), Market::SpotMarket);
            let oracle_meta = AccountMeta { 
                pubkey: spot_market.oracle, 
                is_signer: false, 
                is_writable: false,
            };
            oracle_dict.insert(spot_market.oracle, oracle_meta);
        }
    }

    let oracle_values: Vec<AccountMeta> = oracle_dict.into_values().collect();
    let spot_values: Vec<AccountMeta> = spot_market_dict.into_values().collect();
    let perp_values: Vec<AccountMeta> = perp_market_dict.into_values().collect();
    let remaining_accounts = vec![oracle_values, spot_values, perp_values].concat();

    Ok(remaining_accounts)
}