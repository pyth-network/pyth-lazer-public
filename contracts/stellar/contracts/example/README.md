# Pyth Lazer Stellar Example Consumer

A minimal Soroban contract that demonstrates the end-to-end [Pyth Lazer](https://docs.pyth.network/lazer)
integration on Stellar:

1. **Verify** a signed Lazer update via a deployed [`pyth-lazer-stellar`](../pyth-lazer-stellar) verifier.
2. **Parse** the verified payload with the [`pyth-lazer-stellar-sdk`](../../sdk/rust).
3. **Freshness-check** the feed's update timestamp against a deployment-configured threshold.
4. **Store / retrieve** the latest price for a single configured feed.

This is an example, not a production library — it tracks exactly one feed and keeps only the most
recent price.

## Build & Test

```bash
# From contracts/stellar/
make build                                   # compiles to wasm32v1-none
cargo test --package pyth-lazer-stellar-example
```

## Deploy (testnet)

This example points at the **already-deployed** Pyth Lazer verifier on Stellar testnet — you do not
deploy the verifier yourself:

| Contract | Testnet address |
| -------- | --------------- |
| Pyth Lazer verifier | _TODO: pending redeploy under canonical Pyth-DAO governance_ |

Configure a funded testnet identity once (see the [Stellar CLI docs](https://developers.stellar.org/docs/tools/developer-tools/cli/install-cli)):

```bash
stellar keys generate deployer --network testnet
```

Build and deploy the example, pointing `--lazer` at the verifier above:

```bash
# Build the optimized wasm
make build

# Deploy, passing the constructor args (verifier address, feed id, freshness threshold).
# Here: track BTC/USD (feed id 1) and reject updates older than 60 seconds.
stellar contract deploy \
  --wasm target/wasm32v1-none/release/pyth_lazer_stellar_example.wasm \
  --source deployer \
  --network testnet \
  -- \
  --lazer <LAZER_VERIFIER_CONTRACT_ID> \
  --feed_id 1 \
  --freshness_threshold_us 60000000
```

Then submit a signed Lazer update and read it back:

```bash
stellar contract invoke \
  --id <EXAMPLE_CONTRACT_ADDRESS> \
  --source deployer \
  --network testnet \
  -- update_price --payload <HEX_ENCODED_UPDATE>

stellar contract invoke \
  --id <EXAMPLE_CONTRACT_ADDRESS> \
  --source deployer \
  --network testnet \
  -- get_price
```

## Additional Resources

- The Pyth Lazer Stellar integration guide on [docs.pyth.network/lazer](https://docs.pyth.network/lazer).
