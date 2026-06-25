# pyth-lazer-stellar-sdk

Rust SDK for consuming [Pyth Lazer](https://docs.pyth.network/lazer) price updates inside Soroban (Stellar) smart contracts. It lets a contract cross-contract call a deployed `pyth-lazer-stellar` verifier and decode the verified payload bytes into typed values. The crate is `#![no_std]` and builds for the `wasm32-unknown-unknown` target.

## Install

```toml
[dependencies]
pyth-lazer-stellar-sdk = "0.2"
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

`verify_update` returns a `VerifiedPayload` — a newtype wrapping the decoded
`Update` that signals the bytes passed on-chain verification. Read fields
directly (it derefs to `Update`) or call `.into_inner()` to take ownership of
the `Update`.

> **⚠️ Security: `parse_payload` does not verify signatures.**
>
> `parse_payload` is also exported for callers that already hold verified
> payload bytes. It performs **no** signature verification — it decodes whatever
> bytes it is handed. Only `PythLazerClient::verify_update` establishes the trust
> boundary (it verifies via the on-chain verifier contract before parsing).
> Passing unverified, attacker-supplied bytes to `parse_payload` yields a
> fully-formed but **unsigned** `Update` whose values are entirely
> attacker-controlled and must not be trusted.

## Integration guide

See the full consumer integration guide at
[docs.pyth.network/lazer/price-feeds/pro/integrate-as-consumer/stellar](https://docs.pyth.network/lazer/price-feeds/pro/integrate-as-consumer/stellar).

## Publishing

This crate is published to crates.io via
`.github/workflows/publish-rust-lazer-stellar-sdk.yml`. To cut a new release:

1. Bump `version` in `Cargo.toml` (semver).
2. Commit + merge to `main`.
3. Tag: `git tag rust-pyth-lazer-stellar-sdk-v<X.Y.Z> && git push origin rust-pyth-lazer-stellar-sdk-v<X.Y.Z>`
4. The workflow runs `cargo publish` from the tag. It can also be triggered
   manually via `workflow_dispatch`.
