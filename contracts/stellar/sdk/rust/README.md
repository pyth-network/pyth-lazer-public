# pyth-lazer-stellar-sdk

Rust SDK for consuming [Pyth Lazer](https://docs.pyth.network/lazer) price updates inside Soroban (Stellar) smart contracts. It lets a contract cross-contract call a deployed `pyth-lazer-stellar` verifier and decode the verified payload bytes into typed values. The crate is `#![no_std]` and builds for the `wasm32-unknown-unknown` target.

## Install

```toml
[dependencies]
pyth-lazer-stellar-sdk = "0.1"
```

## Usage

```rust
use pyth_lazer_stellar_sdk::PythLazerClient;
use soroban_sdk::{contract, contractimpl, Address, Bytes, Env};

#[contract]
pub struct ExampleConsumer;

#[contractimpl]
impl ExampleConsumer {
    pub fn update_price(env: Env, lazer: Address, update: Bytes) -> i64 {
        let lazer = PythLazerClient::new(&env, &lazer);
        let parsed = lazer.verify_update(&update).expect("invalid payload");
        parsed
            .feeds
            .iter()
            .find(|f| f.feed_id == 1)
            .and_then(|f| f.price)
            .unwrap_or(0)
    }
}
```

`parse_payload` is also exported for callers that already hold verified payload
bytes and only need to decode them.

## Integration guide

See the full consumer integration guide at
[docs.pyth.network/lazer/price-feeds/pro/integrate-as-consumer/stellar](https://docs.pyth.network/lazer/price-feeds/pro/integrate-as-consumer/stellar).
