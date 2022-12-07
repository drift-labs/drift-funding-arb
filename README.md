## example 
`cargo run -- -k ../keypairs/x19.json -t 1 -s`
- `-t`: 0.1 base size
- `-s`: send transactions to mainnet flag (if not provided will simulate)

## overview
- init drift account 
  - usdc collateral 
- pull market 
- read current market funding rate APY
  - just need market then can use controller/funding.rs math to determine the long/short funding rate 
  - APY calculation = (1 + rate) ^ (24 x 365.25) - 1
    - FUNDING_RATE_BUFFER
- read borrow APR 
  - spot_balance.rs in math/ pub fn calculate_accumulated_interest(
    - SPOT_UTILIZATION_PRECISION
- if funding APY > borrow APR 
  - if funding pays longs -> go long on the perp and borrow (+ sell) SOL spot 
  - if funding pays shorts -> go short on the perp and borrow (+ hold) SOL spot
- closing out = close position + repay spot position 

## todo
- pull and deserialize user perp and spot market data [x]
- compute funding + borrow rates - cross checked with UI [x]
- deposit and withdraw logic (borrow / delta-neutral) [x]
- place_perp_order logic (funding) [x]
- tie it all together [x]
- clean up [x]
- optimize

## optimizations 
- position size needs to ensure will still profit after taker fees in spot and perp for both close and opening position
- only open/change position a small amount of time (~5min) before funding rates are updated
  - could open/close before/after funding if wanted to just capture funding quick