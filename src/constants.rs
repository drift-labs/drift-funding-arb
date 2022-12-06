use anchor_client::solana_sdk::pubkey::Pubkey;
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

lazy_static! {
    pub static ref TOKEN_PROGRAM_ID: Pubkey = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    pub static ref ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey = Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap();
    pub static ref PROGRAM_ID: Pubkey = Pubkey::from_str("dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH").unwrap();
}

#[derive(Debug, Error)]
pub enum DriftError {
    #[error("RpcError {0}")]
    RpcError(#[from] ClientError),
    #[error("AnchorError {0}")]
    AnchorError(#[from] AnchorError),
}

pub type Result<T> = result::Result<T, DriftError>;

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