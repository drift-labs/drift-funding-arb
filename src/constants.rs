use anchor_client::solana_sdk::pubkey::Pubkey;
use std::result;
use std::str::FromStr;
use anchor_client::solana_client::client_error::ClientError;
use thiserror::Error;
use anchor_client::anchor_lang::error::Error as AnchorError;

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
