use std::collections::HashMap;
use std::fmt::Error;
use std::result;
use std::str::FromStr;
use solana_program::instruction::AccountMeta;

use anchor_client::anchor_lang::AccountDeserialize;
use anchor_client::anchor_lang::solana_program::example_mocks::solana_sdk;
use anchor_client::solana_client::nonblocking::pubsub_client::PubsubClient;
use anchor_client::solana_client::rpc_client::RpcClient;
use anchor_client::solana_client::rpc_config::RpcSendTransactionConfig;
use anchor_client::solana_sdk::account_info::AccountInfo;
use anchor_client::solana_sdk::client::AsyncClient;
use anchor_client::solana_sdk::commitment_config::CommitmentConfig;
use anchor_client::solana_sdk::program_error::ProgramError;
use anchor_client::solana_sdk::pubkey::Pubkey;
use anchor_client::solana_sdk::signature::{Keypair, Signer};
use anchor_client::solana_sdk::signature::read_keypair_file;
use drift::instructions::OrderParams;
use drift::state::state::State;
use std::rc::Rc;
use drift::instruction as ix;
use drift::accounts;
use anchor_client::solana_sdk::sysvar;
use anchor_client::solana_sdk::system_program;

use anchor_client::{Client, Cluster, Program};

use drift::controller::funding;
use drift::controller::position::PositionDirection;
use drift::error::DriftResult;
use drift::state::oracle::OraclePriceData;
use drift::state::oracle_map::AccountInfoAndOracleSource;
use drift::state::perp_market::{PerpMarket, self};

use anchor_client::solana_client::client_error::ClientError;
use drift::state::spot_market::{SpotMarket};
use thiserror::Error;
use anchor_client::anchor_lang::error::Error as AnchorError;
use drift::math::casting::Cast;

use drift::math::constants::*;
use drift::math::safe_math::SafeMath;
use std::cmp::{max, min};
use drift::math::spot_balance::{get_token_amount};
use drift::state::spot_market::SpotBalanceType;
use pyth_sdk_solana::{load_price_feed_from_account, PriceFeed, Price};

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

    // setup anchor things 
    let owner = read_keypair_file(owner_kp_path.clone()).unwrap();
    let rc_owner = Rc::new(owner); 
    let provider = Client::new_with_options(
        cluster.clone(), 
        rc_owner.clone(), 
        CommitmentConfig::confirmed() 
    );
    let program = provider.program(PROGRAM_ID.clone());

    let owner = read_keypair_file(owner_kp_path.clone()).unwrap();

    // cache data once for addresses 
    // cache markets once to re-use in get_remaining_accounts
    let subaccount_id: u16 = 0;
    let state = get_state_public_key(&PROGRAM_ID);
    let state_account = get_state(&connection, &state).unwrap();

    let mut cached_accounts: HashMap<Pubkey, Market> = HashMap::new();
    for i in 0..state_account.number_of_markets { 
        let market_pk = get_perp_market_public_key(i, &PROGRAM_ID);
        let market = get_perp_market(&connection, &market_pk)?;
        cached_accounts.insert(market_pk, Market::PerpMarket(market));
    }

    for i in 0..state_account.number_of_spot_markets { 
        let spot_pk = get_spot_market_public_key(i, &PROGRAM_ID);
        let spot_market = get_spot_market(&connection, &spot_pk)?;
        cached_accounts.insert(spot_pk, Market::SpotMarket(spot_market));
    }

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

        if let Some(Market::PerpMarket(market)) = cached_accounts.get(&market_pk) { 
            let oracle_meta = AccountMeta { 
                pubkey: market.amm.oracle, 
                is_signer: false, 
                is_writable: false,
            };
            oracle_dict.insert(market.amm.oracle, oracle_meta);
        } else { panic!("ahh") }
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
            if let Some(Market::SpotMarket(spot_market)) = cached_accounts.get(&spot_pk) {
                let oracle_meta = AccountMeta { 
                    pubkey: spot_market.oracle, 
                    is_signer: false, 
                    is_writable: false,
                };
                oracle_dict.insert(spot_market.oracle, oracle_meta);
            } else { panic!("ahh") }
        }
    }

    let oracle_values: Vec<AccountMeta> = oracle_dict.into_values().collect();
    let spot_values: Vec<AccountMeta> = spot_market_dict.into_values().collect();
    let perp_values: Vec<AccountMeta> = perp_market_dict.into_values().collect();
    let remaining_accounts = vec![oracle_values, spot_values, perp_values].concat();


    // init new account 
    // let name:[u8; 32] = [0; 32];
    // program
    //     .request()
    //     .accounts(accounts::InitializeUser {
    //         user, 
    //         user_stats, 
    //         state, 
    //         authority: owner.pubkey(), 
    //         payer: owner.pubkey(), 
    //         rent: sysvar::rent::id(),
    //         system_program: system_program::id(),
    //     })
    //     .args(ix::InitializeUser {
    //         sub_account_id: subaccount_id, 
    //         name 
    //     })
    //     .send()
    //     .unwrap();

    // program
    //     .request()
    //     .accounts(accounts::InitializeUserStats {
    //         user_stats, 
    //         state, 
    //         authority: owner.pubkey(), 
    //         payer: owner.pubkey(), 
    //         rent: sysvar::rent::id(),
    //         system_program: system_program::id(),
    //     })
    //     .args(ix::InitializeUserStats {})
    //     .send()
    //     .unwrap();






    let market_index: u16 = 0;
    let amount: u64 = 1 * QUOTE_PRECISION as u64;
    let reduce_only = true;

    let spot_market_addr = get_spot_market_public_key(market_index, &PROGRAM_ID);
    let spot_market = get_spot_market(&connection, &spot_market_addr)?;
    let mint = spot_market.mint;

    let user_token_account = derive_token_address(&owner.pubkey(), &mint);

    // place order 
    let order_type = drift::state::user::OrderType::Market;
    // let market_type = drift::state::user::MarketType::Perp;
    let market_type = drift::state::user::MarketType::Spot;
    // let direction = PositionDirection::Long;
    let direction = PositionDirection::Short;
    let market_index = 1; 
    let base_asset_amount = BASE_PRECISION as u64 / 10;
    let reduce_only = false;

    let params = OrderParams { 
        order_type, 
        market_type, 
        direction, 
        base_asset_amount,
        user_order_id: 0, 
        price: 0,
        market_index: market_index,
        reduce_only: reduce_only,
        post_only: false,
        immediate_or_cancel: false,
        trigger_price: None,
        trigger_condition: drift::state::user::OrderTriggerCondition::Above,
        oracle_price_offset: None,
        auction_duration: None,
        max_ts: None,
        auction_start_price: None,
        auction_end_price: None,
    };

    let user = get_user_public_key(&owner.pubkey(), subaccount_id, &PROGRAM_ID);

    let req = program
        .request()
        .accounts(accounts::PlaceOrder { 
            state, 
            user, 
            authority: owner.pubkey()
        })
        .args(ix::PlaceSpotOrder { 
            params
        }).accounts(remaining_accounts);

    // // withdraw + deposit
    // let req = program
    //     .request()
    //     .accounts(accounts::Withdraw {
    //         user,
    //         user_stats, 
    //         state, 
    //         authority: owner.pubkey(), 
    //         spot_market_vault, 
    //         drift_signer,
    //         user_token_account,
    //         token_program: TOKEN_PROGRAM_ID,
    //     }).args(ix::Withdraw { 
    //         market_index, 
    //         amount, 
    //         reduce_only,
    //     }).accounts(remaining_accounts);

    // let req = program
    //     .request()
    //     .accounts(accounts::Deposit {
    //         user,
    //         user_stats, 
    //         state, 
    //         authority: owner.pubkey(), 
    //         spot_market_vault, 
    //         user_token_account,
    //         token_program: TOKEN_PROGRAM_ID,
    //     })
    //     .args(ix::Deposit {
    //         market_index, 
    //         amount, 
    //         reduce_only,
    //     })
    //     .accounts(remaining_accounts);

    let sig = req.send();
    match sig {
        Ok(sig) => println!("sig {}", sig),
        Err(err) => println!("err {:#?}", err),
    };

    // deposit 
    // withdraw 
    // open new position 
    // close position 

    // let market_index = 0; 
    // let market_addr = get_perp_market_public_key(market_index, &program_id);
    // let mut market = get_perp_market(&connection, &market_addr)?;

    // // 1e9 precision
    // let (funding_payment, funding_direction) = compute_funding_rate(&connection, &mut market).unwrap();
    // println!("funding APR: {:#?} {:#?}", funding_payment, funding_direction);

    // let spot_market_index = 1;
    // let spot_market_addr = get_spot_market_public_key(spot_market_index, &program_id);
    // let spot_market = get_spot_market(&connection, &spot_market_addr)?;
    // // 1e4
    // let borrow_rate = compute_borrow_rate(&spot_market).unwrap();
    // println!("borrow APR: {:#?}", borrow_rate);



    Ok(())
}
