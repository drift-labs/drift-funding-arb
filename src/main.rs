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

#[macro_use]
extern crate lazy_static;

mod address;
use address::*;

mod math;
use math::{compute_funding_rate, compute_borrow_rate}; 

mod constants;
use constants::*;


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

pub fn get_order_params(
    order_type: OrderType, 
    market_type: MarketType, 
    direction: PositionDirection, 
    base_asset_amount: u64, 
    market_index: u16, 
    reduce_only: bool,
) -> OrderParams {
    // todo: better auction start/end price
    // start = oracle 
    // end = swap impact 

    OrderParams { 
        order_type, 
        market_type, 
        direction, 
        base_asset_amount,
        market_index,
        reduce_only,
        user_order_id: 0, 
        price: 0,
        post_only: false,
        immediate_or_cancel: false,
        trigger_price: None,
        trigger_condition: drift::state::user::OrderTriggerCondition::Above,
        oracle_price_offset: None,
        auction_duration: None,
        max_ts: None,
        auction_start_price: None,
        auction_end_price: None,
    }
}


fn main() -> Result<()> {
    let cluster_name = "mainnet".to_string(); 
    let cluster = match cluster_name.as_str() {
        "mainnet" => Cluster::Mainnet, 
        _ => panic!("not supported")
    };
    let connection_url = cluster.url();

    let connection = RpcClient::new_with_commitment(
        connection_url,
        CommitmentConfig::confirmed()
    );

    // TODO: make these cli arguments
    let owner_kp_path = "../keypairs/x19zhryYtodTDgmRq6VLtQxbo4zfZUqa9hoobX47BeL.json";
    let subaccount_id: u16 = 0;
    let perp_market_index = 0;
    let spot_market_index = 1;
    let target_position_size = BASE_PRECISION_U64 / 10;
    let simulate = false;
    
    // setup anchor things 
    let owner = read_keypair_file(owner_kp_path.clone()).unwrap();
    let rc_owner = Rc::new(owner); 
    let provider = Client::new_with_options(
        cluster.clone(), 
        rc_owner.clone(), 
        CommitmentConfig::confirmed() 
    );
    let program = provider.program(PROGRAM_ID.clone());

    // cache perp/spot data once for addresses 
    // cache markets once to re-use in get_remaining_accounts
    let state = get_state_public_key(&PROGRAM_ID);
    let state_account = get_state(&connection, &state).unwrap();

    let cached_accounts = get_cached_accounts(&connection, &state_account)?;
    let remaining_accounts = get_remaining_accounts(&state_account, &cached_accounts)?;

    let perp_address = get_perp_market_public_key(perp_market_index, &PROGRAM_ID);
    let mut perp_market = *cast!(cached_accounts.get(&perp_address).unwrap(), Market::PerpMarket);

    // 1e9 precision
    let (funding_payment, funding_direction) = compute_funding_rate(&connection, &mut perp_market).unwrap();
    println!("funding APR: {:#?} {:#?}", funding_payment, funding_direction);

    // 1e9 precision
    let spot_address = get_spot_market_public_key(spot_market_index, &PROGRAM_ID);
    let spot_market = *cast!(cached_accounts.get(&spot_address).unwrap(), Market::SpotMarket);
    let borrow_rate = compute_borrow_rate(&spot_market).unwrap().mul(10_u128.pow(5_u32));
    println!("borrow APR: {:#?}", borrow_rate);

    // todo: check if greater than some threshold (to ensure profit)
    let delta = funding_payment.saturating_sub(borrow_rate);
    println!("INFO: funding delta % {}", delta as f64 / 1e9);
    if delta == 0 { 
        println!("borrow rate too expensive to arb... exiting");
        return Ok(());
    }

    // repay current debt 
    let user_address = get_user_public_key(&rc_owner.pubkey(), subaccount_id, &PROGRAM_ID);
    let user = get_user(&connection, &user_address)?;

    let (target_perp_position, target_spot_position) = match funding_direction { 
        PositionDirection::Long => (PositionDirection::Long, SpotBalanceType::Borrow), 
        PositionDirection::Short => (PositionDirection::Short, SpotBalanceType::Deposit), 
    };
    println!("target perp/spot positions: {:#?} {:#?}", target_perp_position, target_spot_position);

    // adjust position
    // base_amount = if we have a position: 
        // if direction != target_direction: 
            // close current + open in target direction:
            // abs(position) + target position
        // else
            // do nothing 
            // 0
    // else 
        // target position

    let order_base_amount = if let Ok(position) = user.get_perp_position(perp_market_index) { 
        if position.base_asset_amount != 0 && position.get_direction() != target_perp_position {
            println!("PERP: closing current position: {:#?}", position);
            Some(position.base_asset_amount.unsigned_abs() + target_position_size)
        } else { 
            println!("PERP: in correct position, doing nothing...");
            None
        }
    } else { 
        println!("PERP: no current position...");
        Some(target_position_size)
    };

    if let Some(order_base_amount) = order_base_amount { 
        let order_base_amount = standardize_base_asset_amount_ceil(
            order_base_amount, 
            perp_market.amm.order_step_size,
        ).unwrap();

        let params = get_order_params(
            OrderType::Market,
            MarketType::Perp,
            target_perp_position,
            order_base_amount,
            perp_market_index,
            false
        );
        println!("PERP: sending order: {:#?}", params);

        let req = program
            .request()
            .accounts(accounts::PlaceOrder { 
                state, 
                user: user_address.clone(), 
                authority: rc_owner.pubkey()
            })
            .args(ix::PlacePerpOrder { 
                params
            }).accounts(remaining_accounts.clone());
       
        if !simulate { 
            let sig = req.send().unwrap();
            println!("sig {}", sig);
        }
    }

    // adjust spot position
    if !user.is_margin_trading_enabled {
        println!("SPOT: enabling margin trading...");

        // enable margin trading
        let req = program
            .request()
            .accounts(accounts::UpdateUser {
                user: user_address.clone(), 
                authority: rc_owner.pubkey(),
            })
            .args(ix::UpdateUserMarginTradingEnabled {
                _sub_account_id: subaccount_id, 
                margin_trading_enabled: true
            });

        if !simulate { 
            let sig = req.send().unwrap();
            println!("sig {}", sig);
        }
    }

    let spot_order_size = if let Some(position) = user.get_spot_position(spot_market_index) { 
        if position.scaled_balance != 0 && position.balance_type != target_spot_position { 
            println!("SPOT: closing current position: {:#?}", position);
            let token_amount = position.get_signed_token_amount(&spot_market).unwrap();
            Some(token_amount.unsigned_abs() as u64 + target_position_size)
        } else { 
            println!("SPOT: in correct position, doing nothing...");
            None
        }
    } else { 
        println!("SPOT: no current position...");
        Some(target_position_size)
    };

    if let Some(spot_order_size) = spot_order_size { 
        let spot_order_size = standardize_base_asset_amount_ceil(
            spot_order_size, 
            spot_market.order_step_size
        ).unwrap();

        let direction = match target_spot_position { 
            SpotBalanceType::Borrow => PositionDirection::Short, 
            SpotBalanceType::Deposit => PositionDirection::Long,
        };

        let params = get_order_params(
            OrderType::Market,
            MarketType::Spot,
            direction,
            spot_order_size,
            spot_market_index,
            false
        );
        println!("SPOT: sending order: {:#?}", params);

        let req = program
            .request()
            .accounts(accounts::PlaceOrder { 
                state, 
                user: user_address.clone(), 
                authority: rc_owner.pubkey()
            })
            .args(ix::PlaceSpotOrder { 
                params
            }).accounts(remaining_accounts.clone());
        
        if !simulate { 
            let sig = req.send().unwrap();
            println!("sig {}", sig);
        }
    }

    Ok(())
}
