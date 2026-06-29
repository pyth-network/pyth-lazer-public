#!/usr/bin/env bash
set -euo pipefail

# Deploy and initialize Pyth Lazer Stellar contracts to the Stellar network.
#
# Prerequisites:
#   - stellar CLI v25+ (https://developers.stellar.org/docs/tools/cli/install-cli)
#   - Rust >= 1.84 with the wasm32v1-none target: rustup target add wasm32v1-none
#   - wasm-opt (optional, for WASM optimization): cargo install wasm-opt
#
# Usage:
#   ./scripts/deploy.sh --secret <SECRET_KEY> [--network <NETWORK>] [--chain-id <CHAIN_ID>]
#
# Options:
#   --secret              Stellar secret key (S...) or CLI identity alias
#   --network             Stellar network: "testnet" (default) or "mainnet". Both
#                         carry canonical Pyth-DAO defaults for every value below.
#   --chain-id            Pyth receiver chain ID this deployment answers governance
#                         for (testnet 50140 / mainnet 60100). NOT the Wormhole chain
#                         id 30 — see the "Canonical governance configuration" block.
#   --guardian-set        Comma-separated guardian addresses (hex, no 0x prefix).
#                         Defaults to the current Wormhole *mainnet* guardian set on
#                         both networks — see the doc-block below for why.
#   --guardian-index      Guardian set index (default: 4, the mainnet set)
#   --emitter-chain       Governance (owner) emitter chain ID (default: 1 = Solana)
#   --emitter-address     Governance (owner) emitter address (32-byte hex, no 0x prefix)
#   --gs-emitter-chain    Guardian-set-upgrade emitter chain ID (default: 1 = Solana)
#   --gs-emitter-address  Guardian-set-upgrade emitter address (32-byte hex, no 0x prefix)
#   --fund                Fund the account via friendbot (testnet only)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# ─────────────────────────────────────────────────────────────────────────────
# Canonical governance configuration (Pyth-DAO regime)
# ─────────────────────────────────────────────────────────────────────────────
# The executor is authenticated by three (chain-id, address) emitter constants
# plus the Pyth receiver chain id this deployment answers governance for. The
# testnet defaults below are the canonical Pyth-DAO values that every other Pyth
# Lazer contract uses — verify against the cross-referenced sources before any
# (re)deploy.
#
#   1. Owner emitter — authorizes PTGM governance (update_trusted_signer / upgrade)
#        chain id 1 (Solana), address 5635979a…75b12e9e
#      This is the Pyth-DAO Lazer governance emitter: the Wormhole emitter of the
#      Pyth DAO mainnet Squads multisig (FVQyHcooAtThJ83XFrNnv74BcinbRH3bRmfFamAHBfuj),
#      i.e. its Squads authority PDA at authority index 1. It is the same Pyth-DAO
#      governance source for both testnet and mainnet. How to verify:
#        - On-chain: it is the `governance.address` of the live mainnet Pyth Lazer
#          deployments — e.g. the Sui Lazer state object reads chain_id 1 /
#          address 5635979a221c34931e32620b9293a463065555ea71fe97cd6237ade875b12e9e.
#        - Derivable: contract_manager's getMainnetVault().getEmitter() resolves to
#          this value — getAuthorityPDA(<mainnet vault>, 1) under the Squads mesh
#          program (pyth-crosschain/contract_manager/scripts/manage_sui_lazer_contract.ts
#          and src/node/utils/governance.ts).
#
#   2. Guardian-set-upgrade emitter — authorizes guardian set rotations only
#        chain id 1 (Solana), address 0000…0004
#      The Wormhole core bridge governance emitter; identical on every chain.
#
#   3. Pyth receiver chain id (--chain-id) — testnet 50140 / mainnet 60100
#      This is different from Wormhole's Stellar chain id (30) because the contracts
#      have their own receiver implementation. The executor's chain_id MUST match what
#      contract_manager sends, or governance VAAs are rejected. Canonical reference:
#        - pyth-crosschain/contract_manager (xc-admin-common chains.ts):
#          stellar_testnet = 50140, stellar_mainnet = 60100
#
#   4. Wormhole guardian set — the current Wormhole *mainnet* set (index 4), on BOTH
#      networks. The guardians' signatures are what the executor checks on every
#      governance VAA. Because the owner emitter (item 1) lives on Solana *mainnet*,
#      its governance VAAs are signed by the mainnet guardians — so a Stellar testnet
#      deployment must also hold the mainnet set, not the Wormhole testnet guardian.
#      The Lazer Sui testnet deploy does the same: it stands up a custom Wormhole
#      seeded with the mainnet guardians
#      (pyth-crosschain/contract_manager/scripts/deploy_lazer_sui_custom_wormhole.sh).
# ─────────────────────────────────────────────────────────────────────────────

# Defaults
NETWORK="testnet"
CHAIN_ID=""
GUARDIAN_INDEX=""
EMITTER_CHAIN=1
GS_EMITTER_CHAIN=1
FUND=false

# Wormhole core bridge governance emitter (used to sign guardian set upgrades).
# Same on every Wormhole-supported chain and on both networks.
GS_EMITTER_ADDRESS_DEFAULT="0000000000000000000000000000000000000000000000000000000000000004"
# Pyth-DAO Lazer governance (owner) emitter — chain 1 (Solana). Same for both
# networks (see the "Canonical governance configuration" doc-block above).
PYTH_DAO_LAZER_OWNER_EMITTER="5635979a221c34931e32620b9293a463065555ea71fe97cd6237ade875b12e9e"

# Wormhole guardian set — the current Wormhole *mainnet* set (index 4, 19 guardians).
# This is used on BOTH Stellar testnet and mainnet: Pyth-DAO Lazer governance VAAs are
# emitted by the Pyth DAO on Solana *mainnet* and signed by the mainnet Wormhole
# guardians, so every Lazer deployment must verify against the mainnet set regardless of
# which network the contract lives on. (This mirrors the Lazer Sui testnet deploy, which
# stands up a custom Wormhole seeded with the mainnet guardians — see
# pyth-crosschain/contract_manager/scripts/deploy_lazer_sui_custom_wormhole.sh.)
# Refresh from: curl "https://api.wormholescan.io/api/v1/governor/config" | jq -r '[.data[].id] | join(",")'
WORMHOLE_GUARDIAN_SET="5893B5A76c3f739645648885bDCcC06cd70a3Cd3,fF6CB952589BDE862c25Ef4392132fb9D4A42157,114De8460193bdf3A2fCf81f86a09765F4762fD1,107A0086b32d7A0977926A205131d8731D39cbEB,8C82B2fd82FaeD2711d59AF0F2499D16e726f6b2,11b39756C042441BE6D8650b69b54EbE715E2343,54Ce5B4D348fb74B958e8966e2ec3dBd4958a7cd,15e7cAF07C4e3DC8e7C469f92C8Cd88FB8005a20,74a3bf913953D695260D88BC1aA25A4eeE363ef0,000aC0076727b35FBea2dAc28fEE5cCB0fEA768e,AF45Ced136b9D9e24903464AE889F5C8a723FC14,f93124b7c738843CBB89E864c862c38cddCccF95,D2CC37A4dc036a8D232b48f62cDD4731412f4890,DA798F6896A3331F64b48c12D1D57Fd9cbe70811,71AA1BE1D36CaFE3867910F99C09e347899C19C3,8192b6E7387CCd768277c17DAb1b7a5027c0b3Cf,178e21ad2E77AE06711549CFBB1f9c7a9d8096e8,5E1487F35515d02A92753504a8D75471b9f49EdB,6FbEBc898F403E4773E95feB15E80C9A99c8348d"
WORMHOLE_GUARDIAN_SET_INDEX=4

# Testnet defaults (Pyth-DAO canonical — see the doc-block above)
TESTNET_GUARDIAN="$WORMHOLE_GUARDIAN_SET"
TESTNET_GUARDIAN_INDEX="$WORMHOLE_GUARDIAN_SET_INDEX"
# Pyth receiver chain id for Stellar testnet (NOT the Wormhole chain id 30).
TESTNET_CHAIN_ID=50140
TESTNET_EMITTER_ADDRESS="$PYTH_DAO_LAZER_OWNER_EMITTER"
TESTNET_GS_EMITTER_ADDRESS="$GS_EMITTER_ADDRESS_DEFAULT"

# Mainnet defaults (Pyth-DAO canonical — see the doc-block above)
MAINNET_GUARDIAN="$WORMHOLE_GUARDIAN_SET"
MAINNET_GUARDIAN_INDEX="$WORMHOLE_GUARDIAN_SET_INDEX"
# Pyth receiver chain id for Stellar mainnet (NOT the Wormhole chain id 30).
MAINNET_CHAIN_ID=60100
MAINNET_EMITTER_ADDRESS="$PYTH_DAO_LAZER_OWNER_EMITTER"
MAINNET_GS_EMITTER_ADDRESS="$GS_EMITTER_ADDRESS_DEFAULT"

# Pyth Lazer trusted signer public key (compressed secp256k1, hex)
LAZER_SIGNER_PUBKEY="03a4380f01136eb2640f90c17e1e319e02bbafbeef2e6e67dc48af53f9827e155b"
FAR_FUTURE_EXPIRY=9999999999

SECRET=""
GUARDIAN_SET=""
EMITTER_ADDRESS=""
GS_EMITTER_ADDRESS=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --secret)
            SECRET="$2"; shift 2 ;;
        --network)
            NETWORK="$2"; shift 2 ;;
        --chain-id)
            CHAIN_ID="$2"; shift 2 ;;
        --guardian-set)
            GUARDIAN_SET="$2"; shift 2 ;;
        --guardian-index)
            GUARDIAN_INDEX="$2"; shift 2 ;;
        --emitter-chain)
            EMITTER_CHAIN="$2"; shift 2 ;;
        --emitter-address)
            EMITTER_ADDRESS="$2"; shift 2 ;;
        --gs-emitter-chain)
            GS_EMITTER_CHAIN="$2"; shift 2 ;;
        --gs-emitter-address)
            GS_EMITTER_ADDRESS="$2"; shift 2 ;;
        --fund)
            FUND=true; shift ;;
        *)
            echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [[ -z "$SECRET" ]]; then
    echo "Error: --secret is required"
    exit 1
fi

# Apply per-network Pyth-DAO canonical defaults for any value not overridden on
# the command line.
if [[ "$NETWORK" == "testnet" ]]; then
    : "${CHAIN_ID:=$TESTNET_CHAIN_ID}"
    : "${GUARDIAN_SET:=$TESTNET_GUARDIAN}"
    : "${GUARDIAN_INDEX:=$TESTNET_GUARDIAN_INDEX}"
    : "${EMITTER_ADDRESS:=$TESTNET_EMITTER_ADDRESS}"
    : "${GS_EMITTER_ADDRESS:=$TESTNET_GS_EMITTER_ADDRESS}"
elif [[ "$NETWORK" == "mainnet" ]]; then
    : "${CHAIN_ID:=$MAINNET_CHAIN_ID}"
    : "${GUARDIAN_SET:=$MAINNET_GUARDIAN}"
    : "${GUARDIAN_INDEX:=$MAINNET_GUARDIAN_INDEX}"
    : "${EMITTER_ADDRESS:=$MAINNET_EMITTER_ADDRESS}"
    : "${GS_EMITTER_ADDRESS:=$MAINNET_GS_EMITTER_ADDRESS}"
fi

# Guardian set index defaults to 0 for any network without a canonical default.
: "${GUARDIAN_INDEX:=0}"

if [[ -z "$CHAIN_ID" ]]; then
    echo "Error: --chain-id (Pyth receiver chain id) is required for unknown networks"
    exit 1
fi
if [[ -z "$GUARDIAN_SET" ]]; then
    echo "Error: --guardian-set is required for unknown networks"
    exit 1
fi
if [[ -z "$EMITTER_ADDRESS" ]]; then
    echo "Error: --emitter-address is required for unknown networks"
    exit 1
fi
if [[ -z "$GS_EMITTER_ADDRESS" ]]; then
    echo "Error: --gs-emitter-address is required for unknown networks"
    exit 1
fi

COMMON_ARGS="--network $NETWORK --source $SECRET"

echo "=== Pyth Lazer Stellar Deployment ==="
echo "Network:                  $NETWORK"
echo "Pyth Receiver Chain ID:   $CHAIN_ID"
echo "Guardian Index:           $GUARDIAN_INDEX"
echo "Owner Emitter:            chain $EMITTER_CHAIN / $EMITTER_ADDRESS"
echo "GS Upgrade Emitter:       chain $GS_EMITTER_CHAIN / $GS_EMITTER_ADDRESS"
echo ""

# Step 0: Fund account via friendbot (testnet only)
if [[ "$FUND" == "true" && "$NETWORK" == "testnet" ]]; then
    echo "Funding account via friendbot..."
    stellar keys fund "$SECRET" --network testnet 2>/dev/null || \
        echo "Warning: could not fund account (may already be funded)"
    echo ""
fi

# Step 1: Build contracts
echo "=== Building contracts ==="
cd "$WORKSPACE_DIR"
cargo build --release --target wasm32v1-none -p wormhole-executor-stellar -p pyth-lazer-stellar
echo "Build complete."
echo ""

WASM_DIR="$WORKSPACE_DIR/target/wasm32v1-none/release"
EXECUTOR_WASM="$WASM_DIR/wormhole_executor_stellar.wasm"
LAZER_WASM="$WASM_DIR/pyth_lazer_stellar.wasm"

# Optimize WASM files using wasm-opt if available
# This strips reference-types (required for Soroban VM) and reduces size
if command -v wasm-opt &> /dev/null; then
    echo "=== Optimizing WASM files with wasm-opt ==="
    wasm-opt -Oz --disable-reference-types \
        "$EXECUTOR_WASM" -o "${EXECUTOR_WASM%.wasm}.optimized.wasm"
    EXECUTOR_WASM="${EXECUTOR_WASM%.wasm}.optimized.wasm"

    wasm-opt -Oz --disable-reference-types \
        "$LAZER_WASM" -o "${LAZER_WASM%.wasm}.optimized.wasm"
    LAZER_WASM="${LAZER_WASM%.wasm}.optimized.wasm"
else
    echo "Warning: wasm-opt not found. Install with: cargo install wasm-opt"
    echo "Deploying unoptimized WASM (may fail if reference-types are present)."
fi

echo "Executor WASM: $EXECUTOR_WASM ($(wc -c < "$EXECUTOR_WASM") bytes)"
echo "Lazer WASM:    $LAZER_WASM ($(wc -c < "$LAZER_WASM") bytes)"
echo ""

# Step 2: Prepare constructor args
echo "=== Preparing constructor arguments ==="

# Build guardian set JSON array
IFS=',' read -ra GUARDIANS <<< "$GUARDIAN_SET"
GUARDIAN_JSON="["
for i in "${!GUARDIANS[@]}"; do
    addr="${GUARDIANS[$i]}"
    addr=$(echo "$addr" | tr '[:upper:]' '[:lower:]' | sed 's/^0x//')
    if [[ $i -gt 0 ]]; then
        GUARDIAN_JSON+=","
    fi
    GUARDIAN_JSON+="\"$addr\""
done
GUARDIAN_JSON+="]"

# Pad emitter addresses to 64 hex chars (32 bytes).
PADDED_EMITTER=$(printf '%064s' "$EMITTER_ADDRESS" | tr ' ' '0')
PADDED_GS_EMITTER=$(printf '%064s' "$GS_EMITTER_ADDRESS" | tr ' ' '0')

# Step 3: Deploy wormhole-executor-stellar (runs __constructor during deploy)
echo "=== Deploying wormhole-executor-stellar ==="
EXECUTOR_ID=$(stellar contract deploy \
    --wasm "$EXECUTOR_WASM" \
    $COMMON_ARGS \
    -- \
    --chain_id "$CHAIN_ID" \
    --owner_emitter_chain "$EMITTER_CHAIN" \
    --owner_emitter_address "$PADDED_EMITTER" \
    --gs_upgrade_emitter_chain "$GS_EMITTER_CHAIN" \
    --gs_upgrade_emitter_address "$PADDED_GS_EMITTER" \
    --initial_guardian_set "$GUARDIAN_JSON" \
    --guardian_set_index "$GUARDIAN_INDEX" \
    2>&1 | tail -1)
echo "Executor contract ID: $EXECUTOR_ID"
echo ""

# Step 4: Deploy pyth-lazer-stellar with initial trusted signer (__constructor)
echo "=== Deploying pyth-lazer-stellar ==="
LAZER_ID=$(stellar contract deploy \
    --wasm "$LAZER_WASM" \
    $COMMON_ARGS \
    -- \
    --executor "$EXECUTOR_ID" \
    --initial_signer "\"$LAZER_SIGNER_PUBKEY\"" \
    --initial_signer_expires_at "$FAR_FUTURE_EXPIRY" \
    2>&1 | tail -1)
echo "Lazer contract ID: $LAZER_ID"
echo "Trusted signer: $LAZER_SIGNER_PUBKEY (expires: $FAR_FUTURE_EXPIRY)"
echo ""

# Output summary
echo "========================================="
echo "=== Deployment Complete ==="
echo "========================================="
echo ""
echo "Network:                  $NETWORK"
echo "Pyth Receiver Chain ID:   $CHAIN_ID"
echo "Guardian Set Index:       $GUARDIAN_INDEX"
echo ""
echo "Canonical governance config (self-audit against pyth-crosschain):"
echo "  Owner Emitter:          chain $EMITTER_CHAIN / $PADDED_EMITTER"
echo "  GS Upgrade Emitter:     chain $GS_EMITTER_CHAIN / $PADDED_GS_EMITTER"
echo "  Guardian Set:           $GUARDIAN_JSON"
echo ""
echo "Executor Contract ID:   $EXECUTOR_ID"
echo "Lazer Contract ID:      $LAZER_ID"
echo ""
echo "Lazer Signer Pubkey: $LAZER_SIGNER_PUBKEY"
echo "Signer Expiry:      $FAR_FUTURE_EXPIRY"
echo ""
echo "Next steps:"
echo "  1. Call verify_update on the Lazer contract with a signed price update"
echo ""
echo "Example - verify update:"
echo "  stellar contract invoke --id $LAZER_ID --network $NETWORK --source <SECRET> -- \\"
echo "    verify_update --data <SIGNED_UPDATE_HEX>"
