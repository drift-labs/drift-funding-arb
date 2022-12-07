use std::ops::Mul;
use std::rc::Rc;

use drift::math::position::direction_to_close_position;
use anchor_client::solana_client::rpc_client::RpcClient;
use anchor_client::solana_sdk::commitment_config::CommitmentConfig;
use anchor_client::solana_sdk::signature::Signer;
use anchor_client::solana_sdk::signature::read_keypair_file;
use anchor_client::{Client, Cluster};

use drift::state::user::{OrderType, MarketType};
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

// deriving pdas + getting accounts
mod address;
use address::*;

// funding + borrow + oracle stuff
mod math;
use math::{compute_funding_rate, compute_borrow_rate}; 

// Results<> + macros
mod constants;
use constants::*;

// caching accounts + remaining_accounts
#[macro_use]
mod utils; 
use utils::*;

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

use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// keypair for owner 
    #[clap(long, short)]
    keypair_path: String,
    /// position size of the arb  (with precision 10)
    #[clap(long, short)]
    target_position_size: u64,
    /// subaccount id of owner
    #[clap(long, default_value_t = 0)]
    subaccount_id: u16,
    /// perp to long/short for funding
    #[clap(long, default_value_t = 0)]
    perp_market_index: u16,
    /// spot to long/short for delta-neutral position
    #[clap(long, default_value_t = 1)]
    spot_market_index: u16,
    /// simulate what would happen
    #[clap(long, short, action)]
    simulate: bool,
}

fn main() -> Result<()> {
    let Args { 
        keypair_path, 
        subaccount_id, 
        perp_market_index, 
        spot_market_index, 
        mut target_position_size, 
        mut simulate,
    } = Args::parse();

    simulate = !simulate; // will simulate by default -- provde '-s' flag to do real
    target_position_size *= BASE_PRECISION_U64 / 10;

    // setup rpc 
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
    
    // setup anchor things 
    let owner = read_keypair_file(keypair_path.clone()).unwrap();
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

    let spot_address = get_spot_market_public_key(spot_market_index, &PROGRAM_ID);
    let spot_market = *cast!(cached_accounts.get(&spot_address).unwrap(), Market::SpotMarket);

    let spot_name = String::from_utf8_lossy(&spot_market.name);
    let perp_name = String::from_utf8_lossy(&perp_market.name);
    let _spot = spot_name.trim();
    let _perp = perp_name.trim().split("-").collect::<Vec<&str>>()[0];
    println!("spot/perp name: {} {}", _spot, _perp);
    if _spot != _perp {
        println!("spot/perp name dont match ... exiting");
        return Ok(())
    }

    // 1e9 precision
    let (funding_payment, funding_direction) = compute_funding_rate(&connection, &mut perp_market).unwrap();
    println!("funding APR: {:#?} {:#?}", funding_payment, funding_direction);

    // 1e9 precision
    let borrow_rate = compute_borrow_rate(&spot_market).unwrap().mul(10_u128.pow(5_u32));
    println!("borrow APR: {:#?}", borrow_rate);

    // todo: check if greater than some threshold (to ensure profit)
    let delta = funding_payment.saturating_sub(borrow_rate);
    println!("INFO: funding delta % {}", delta as f64 / 1e9);

    let should_close_position = delta == 0;
    if should_close_position { 
        println!("borrow rate too expensive to arb... closing positions");
    }

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

    let perp_order = if let Ok(position) = user.get_perp_position(perp_market_index) { 
        if should_close_position {
            Some((
                position.base_asset_amount.unsigned_abs(), 
                direction_to_close_position(position.base_asset_amount.into())
            ))
        } else if position.base_asset_amount != 0 && position.get_direction() != target_perp_position {
            println!("PERP: closing current position: {:#?}", position);
            Some((
                position.base_asset_amount.unsigned_abs() + target_position_size, 
                target_perp_position
            ))
        } else { 
            println!("PERP: in correct position, doing nothing...");
            None
        }
    } else { 
        println!("PERP: no current position...");
        Some((target_position_size, target_perp_position))
    };

    if let Some((order_base_amount, direction)) = perp_order { 
        let order_base_amount = standardize_base_asset_amount_ceil(
            order_base_amount, 
            perp_market.amm.order_step_size,
        ).unwrap();

        let params = get_order_params(
            OrderType::Market,
            MarketType::Perp,
            direction,
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

    let spot_order = if let Some(position) = user.get_spot_position(spot_market_index) { 
        let token_amount = position.get_signed_token_amount(&spot_market).unwrap();

        if should_close_position { 
            let direction_to_close = match target_spot_position { 
                SpotBalanceType::Borrow => PositionDirection::Long, 
                SpotBalanceType::Deposit => PositionDirection::Short,
            };
            Some((
                token_amount.unsigned_abs() as u64, 
                direction_to_close
            ))
        } else if position.scaled_balance != 0 && position.balance_type != target_spot_position { 
            println!("SPOT: closing current position: {:#?}", position);
            let direction = match target_spot_position { 
                SpotBalanceType::Borrow => PositionDirection::Short, 
                SpotBalanceType::Deposit => PositionDirection::Long,
            };
            Some((token_amount.unsigned_abs() as u64 + target_position_size, direction))
        } else { 
            println!("SPOT: in correct position, doing nothing...");
            None
        }
    } else { 
        let direction = match target_spot_position { 
            SpotBalanceType::Borrow => PositionDirection::Short, 
            SpotBalanceType::Deposit => PositionDirection::Long,
        };
        println!("SPOT: no current position...");
        Some((target_position_size, direction))
    };

    if let Some((spot_order_size, direction)) = spot_order { 
        let spot_order_size = standardize_base_asset_amount_ceil(
            spot_order_size, 
            spot_market.order_step_size
        ).unwrap();

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
