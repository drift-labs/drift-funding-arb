use std::cmp::{max, min};

use anchor_client::solana_client::rpc_client::RpcClient;
use anchor_client::solana_sdk::pubkey::Pubkey;

use drift::controller::position::PositionDirection;
use drift::error::DriftResult;
use drift::math::casting::Cast;
use drift::math::constants::*;
use drift::math::safe_math::SafeMath;
use drift::math::spot_balance::{get_token_amount};
use drift::state::oracle::OraclePriceData;
use drift::state::perp_market::PerpMarket;
use drift::state::spot_market::{SpotMarket};
use drift::state::spot_market::SpotBalanceType;

use pyth_sdk_solana::{load_price_feed_from_account, PriceFeed, Price};

fn get_oracle_info(connection: &RpcClient, oracle_pk: &Pubkey) -> DriftResult<OraclePriceData> {
    let mut account = connection.get_account(oracle_pk).unwrap();
    let price_feed: PriceFeed = load_price_feed_from_account(&oracle_pk, &mut account).unwrap();
    let price_data: Price = price_feed.get_current_price().unwrap();
    let oracle_price = price_data.price;
    let oracle_conf = price_data.conf;

    let oracle_precision = 10_u128.pow(price_data.expo.unsigned_abs());

    let mut oracle_scale_mult = 1;
    let mut oracle_scale_div = 1;

    if oracle_precision > PRICE_PRECISION {
        oracle_scale_div = oracle_precision.safe_div(PRICE_PRECISION)?;
    } else {
        oracle_scale_mult = PRICE_PRECISION.safe_div(oracle_precision)?;
    }

    let oracle_price_scaled = (oracle_price)
        .cast::<i128>()?
        .safe_mul(oracle_scale_mult.cast()?)?
        .safe_div(oracle_scale_div.cast()?)?
        .cast::<i64>()?;

    let oracle_conf_scaled = (oracle_conf)
        .cast::<u128>()?
        .safe_mul(oracle_scale_mult)?
        .safe_div(oracle_scale_div)?
        .cast::<u64>()?;

    Ok(OraclePriceData {
        price: oracle_price_scaled,
        confidence: oracle_conf_scaled,
        delay: 0,
        has_sufficient_number_of_data_points: true,
    })
}

// v2/controller/funding.rs
// v2/math/funding.rs
pub fn compute_funding_rate(connection: &RpcClient, market: &mut PerpMarket) -> DriftResult<(u128, PositionDirection)> { 
    let oracle_pk = market.amm.oracle;
    let oracle_price_data = get_oracle_info(&connection, &oracle_pk).unwrap();
    let sanitize_clamp_denominator = market.get_sanitize_clamp_denominator().unwrap();

    let slot = connection.get_slot().unwrap();
    let now = connection.get_block_time(slot).unwrap();
    let reserve_price = market.amm.reserve_price().unwrap();
    let oracle_price_twap = drift::math::amm::update_oracle_price_twap(
        &mut market.amm,
        now,
        &oracle_price_data,
        Some(reserve_price),
        sanitize_clamp_denominator,
    ).unwrap();

    // price relates to execution premium / direction
    let (execution_premium_price, execution_premium_direction) =
        if market.amm.long_spread > market.amm.short_spread {
            (
                market.amm.ask_price(reserve_price)?,
                Some(PositionDirection::Long),
            )
        } else if market.amm.long_spread < market.amm.short_spread {
            (
                market.amm.bid_price(reserve_price)?,
                Some(PositionDirection::Short),
            )
        } else {
            (reserve_price, None)
        };

    let sanitize_clamp_denominator = market.get_sanitize_clamp_denominator()?;
    let mid_price_twap = drift::math::amm::update_mark_twap(
        &mut market.amm,
        now,
        Some(execution_premium_price),
        execution_premium_direction,
        sanitize_clamp_denominator,
    )?;

    let period_adjustment = (24_i128)
        .safe_mul(ONE_HOUR_I128)?
        .safe_div(max(ONE_HOUR_I128, market.amm.funding_period as i128))?;

    // funding period = 1 hour, window = 1 day
    // low periodicity => quickly updating/settled funding rates => lower funding rate payment per interval
    let price_spread = mid_price_twap.cast::<i64>()?.safe_sub(oracle_price_twap)?;

    // clamp price divergence to 3% for funding rate calculation
    let max_price_spread = oracle_price_twap.safe_div(33)?; // 3%
    let clamped_price_spread = max(-max_price_spread, min(price_spread, max_price_spread));

    let funding_rate = clamped_price_spread
        .cast::<i128>()?
        .safe_mul(FUNDING_RATE_BUFFER.cast()?)?
        .safe_div(period_adjustment.cast()?)?
        .cast::<i64>()?;

    let (funding_rate_long, funding_rate_short, _) =
        drift::math::funding::calculate_funding_rate_long_short(market, funding_rate.cast()?)?;

    let (funding_delta, funding_direction) = if mid_price_twap.cast::<i64>()? > oracle_price_twap { 
        (funding_rate_short, PositionDirection::Short)
    } else { 
        (funding_rate_long, PositionDirection::Long)
    };

    // 1e9 precision
    let funding_rate = funding_delta
        .safe_mul(PRICE_PRECISION_I128)?
        .safe_div(oracle_price_twap.cast()?)?
        .unsigned_abs();

    let funding_apr = funding_rate
        .safe_mul(100)?
        .safe_mul(24)?
        .safe_mul(365)?;

    Ok((funding_apr, funding_direction))
}


pub fn compute_borrow_rate(spot_market: &SpotMarket) -> DriftResult<u128> {
    let deposit_token_amount = get_token_amount(
        spot_market.deposit_balance,
        spot_market,
        &SpotBalanceType::Deposit,
    )?;
    let borrow_token_amount = get_token_amount(
        spot_market.borrow_balance,
        spot_market,
        &SpotBalanceType::Borrow,
    )?;

    let utilization = drift::math::spot_balance::calculate_utilization(deposit_token_amount, borrow_token_amount)?;

    if utilization == 0 {
        return Ok(0);
    }

    let borrow_rate = if utilization > spot_market.optimal_utilization.cast()? {
        let surplus_utilization = utilization.safe_sub(spot_market.optimal_utilization.cast()?)?;

        let borrow_rate_slope = spot_market
            .max_borrow_rate
            .cast::<u128>()?
            .safe_sub(spot_market.optimal_borrow_rate.cast()?)?
            .safe_mul(SPOT_UTILIZATION_PRECISION)?
            .safe_div(
                SPOT_UTILIZATION_PRECISION.safe_sub(spot_market.optimal_utilization.cast()?)?,
            )?;

        spot_market.optimal_borrow_rate.cast::<u128>()?.safe_add(
            surplus_utilization
                .safe_mul(borrow_rate_slope)?
                .safe_div(SPOT_UTILIZATION_PRECISION)?,
        )?
    } else {
        let borrow_rate_slope = spot_market
            .optimal_borrow_rate
            .cast::<u128>()?
            .safe_mul(SPOT_UTILIZATION_PRECISION)?
            .safe_div(spot_market.optimal_utilization.cast()?)?;

        utilization
            .safe_mul(borrow_rate_slope)?
            .safe_div(SPOT_UTILIZATION_PRECISION)?
    };

    Ok(borrow_rate)
}