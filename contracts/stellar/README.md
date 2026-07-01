# Pyth Lazer Stellar Contracts

Soroban smart contracts for verifying [Pyth Lazer](https://docs.pyth.network/lazer) price feed updates on the [Stellar](https://stellar.org) network.

## Architecture

```
                        Wormhole Guardians
                              |
                         sign VAAs
                              |
                              v
┌──────────────────────────────────────────┐
│         wormhole-executor-stellar        │
│                                          │
│  - Verifies Wormhole VAA signatures      │
│  - Manages guardian sets                 │
│  - Parses PTGM governance payloads       │
│  - Dispatches governance to target       │
└────────────────┬─────────────────────────┘
                 │ cross-contract call
                 │ (update_trusted_signer / upgrade)
                 v
┌──────────────────────────────────────────┐
│          pyth-lazer-stellar              │
│                                          │
│  - Verifies LE-ECDSA signed updates      │
│  - Manages trusted signers (via executor)│
│  - Parses structured price feed payloads │
│  - Returns verified payload to caller    │
└──────────────────────────────────────────┘
                 ^
                 │ verify_update(data)
                 │
            Consumer dApp
```

**Flow:**
1. Wormhole governance VAAs add/remove trusted Lazer signers via the executor
2. Lazer publishes LE-ECDSA signed price updates off-chain
3. Consumer dApps submit updates to `pyth-lazer-stellar.verify_update()` to get verified price data

## Prerequisites

- [Rust](https://rustup.rs/) (>= 1.84)
- Soroban target: `rustup target add wasm32v1-none`
- [Stellar CLI](https://developers.stellar.org/docs/tools/developer-tools/cli/install-cli) (for deployment)

## Build

```bash
make build
```

This compiles both contracts to WASM targeting `wasm32v1-none`.

## Test

```bash
make test
```

Runs all unit tests and integration tests. The integration test suite (`contracts/integration-tests`) exercises:

- Full governance flow: VAA -> executor -> Lazer contract (add/update/remove signers)
- Full verification flow: signed Lazer update -> verify -> parse payload (BTC, ETH, SOL feeds)
- Upgrade governance dispatch
- Guardian set upgrades followed by governance actions
- Negative cases: expired signers, wrong emitter, replayed VAAs, unauthorized calls, invalid PTGM

## End-to-End Test

Run the E2E test against the testnet deployment:

```bash
cd scripts/e2e
PYTH_LAZER_TOKEN=<your-token> npx tsx src/test_real_update.ts \
  --contract-id CAYFT5JE3UQTKT4Q6ZOZK4FXVYVT6RE3MFC7STA4UB6WAEGBT65MRU52   # see the Testnet Deployment table below
```

This fetches a real signed price update from the Pyth Lazer service and verifies it on-chain.

## Code Quality

```bash
make fmt        # Format code
make fmt-check  # Check formatting (CI)
make clippy     # Run clippy lints
make check      # fmt-check + clippy + test
```

## Project Structure

```
lazer/contracts/stellar/
├── Cargo.toml                          # Workspace root
├── Makefile
├── README.md
├── contracts/
│   ├── pyth-lazer-stellar/             # Lazer verification contract
│   │   └── src/
│   │       ├── lib.rs                  # Contract entry points
│   │       ├── verify.rs               # LE-ECDSA signature verification
│   │       ├── payload.rs              # Price feed payload parsing
│   │       ├── state.rs                # Storage management
│   │       └── error.rs                # Error types
│   ├── wormhole-executor-stellar/      # Wormhole governance executor
│   │   └── src/
│   │       ├── lib.rs                  # Contract entry points
│   │       ├── vaa.rs                  # VAA parsing & verification
│   │       ├── governance.rs           # PTGM parsing
│   │       ├── guardian.rs             # Guardian set management
│   │       └── error.rs               # Error types
│   └── integration-tests/             # Cross-contract integration tests
│       └── src/
│           └── test.rs
```

## Testnet Deployment

| Contract | Address |
|----------|---------|
| Pyth Lazer (verifier) | [`CAYFT5JE3UQTKT4Q6ZOZK4FXVYVT6RE3MFC7STA4UB6WAEGBT65MRU52`](https://stellar.expert/explorer/testnet/contract/CAYFT5JE3UQTKT4Q6ZOZK4FXVYVT6RE3MFC7STA4UB6WAEGBT65MRU52) |
| Wormhole Executor | [`CA542YLVDLBQXTTS2FOERQAED2WE5DQRKLRZUUEG75DQ2TEX2525QNBG`](https://stellar.expert/explorer/testnet/contract/CA542YLVDLBQXTTS2FOERQAED2WE5DQRKLRZUUEG75DQ2TEX2525QNBG) |

The testnet contract is initialized with the canonical Pyth-DAO Lazer governance emitter and the
Pyth Lazer trusted signer (see the governance config baked into `scripts/deploy.sh`). The canonical
machine-readable record of the deployed contract ids lives in the `contract_manager` registry in
`pyth-network/pyth-crosschain`.

## Deployment

Use `scripts/deploy.sh`. It builds, deploys, and initializes both contracts with the canonical
Pyth-DAO governance defaults (owner emitter, guardian set, and Pyth receiver chain id) for the chosen
network — no governance overrides are needed. See the "Canonical governance configuration" doc-block
at the top of the script for what those defaults are and how to verify them.

```bash
# Testnet (--fund friendbot-funds a fresh account)
./scripts/deploy.sh --secret <SECRET_KEY_OR_IDENTITY> --network testnet --fund

# Mainnet
./scripts/deploy.sh --secret <SECRET_KEY_OR_IDENTITY> --network mainnet
```

## Configuration

| Parameter | Description |
|-----------|-------------|
| `chain_id` | Pyth receiver chain ID this deployment answers governance for: testnet `50140` / mainnet `60100`. This is different from Wormhole's Stellar chain id (30) because the contracts have their own receiver implementation. Must match what `contract_manager` sends |
| `owner_emitter_chain` | Wormhole chain ID of the Pyth governance emitter (1 = Solana) |
| `owner_emitter_address` | 32-byte Wormhole emitter address of the Pyth-DAO Lazer governance emitter (`5635979a…75b12e9e`) — the Squads authority PDA (index 1) of the Pyth DAO mainnet multisig; same for testnet and mainnet |
| `gs_upgrade_emitter_chain` | Wormhole chain ID of the Wormhole core bridge governance emitter (1 = Solana). Authorizes guardian set upgrades only |
| `gs_upgrade_emitter_address` | 32-byte Wormhole emitter address used to authorize guardian set upgrades |
| `initial_guardian_set` | List of 20-byte Ethereum addresses of Wormhole guardians. Use the current Wormhole **mainnet** guardian set on both networks — governance VAAs are signed by the mainnet guardians since the owner emitter lives on Solana mainnet |
| `guardian_set_index` | Wormhole guardian set index — `7` (the current mainnet set) on both networks |

See [Wormhole contract addresses](https://wormhole.com/docs/products/reference/contract-addresses) and [chain IDs](https://wormhole.com/docs/products/reference/chain-ids) for reference values.
