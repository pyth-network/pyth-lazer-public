extern crate alloc;
extern crate std;

use k256::ecdsa::SigningKey;
use soroban_sdk::{testutils::Address as _, testutils::Ledger, Address, Bytes, BytesN, Env, Vec};
use tiny_keccak::{Hasher, Keccak};

use pyth_lazer_stellar::{
    payload, ContractError as LazerError, PythLazerContract, PythLazerContractClient,
};
use wormhole_executor_stellar::error::ContractError as ExecutorError;
use wormhole_executor_stellar::{WormholeExecutor, WormholeExecutorClient};

// ──────────────────────────────────────────────────────────────────────
// Crypto helpers (shared with wormhole-executor-stellar unit tests)
// ──────────────────────────────────────────────────────────────────────

fn test_secret(index: u8) -> [u8; 32] {
    let mut secret = [0u8; 32];
    secret[31] = index + 1;
    secret
}

fn eth_address_from_secret(secret: &[u8; 32]) -> [u8; 20] {
    let signing_key = SigningKey::from_bytes(secret.into()).expect("valid key");
    let verifying_key = signing_key.verifying_key();
    let uncompressed = verifying_key.to_encoded_point(false);
    let pubkey_bytes = &uncompressed.as_bytes()[1..];

    let mut hasher = Keccak::v256();
    let mut hash = [0u8; 32];
    hasher.update(pubkey_bytes);
    hasher.finalize(&mut hash);

    let mut addr = [0u8; 20];
    addr.copy_from_slice(&hash[12..32]);
    addr
}

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak::v256();
    let mut out = [0u8; 32];
    hasher.update(data);
    hasher.finalize(&mut out);
    out
}

fn sign_hash(secret: &[u8; 32], hash: &[u8; 32]) -> ([u8; 64], u32) {
    let signing_key = SigningKey::from_bytes(secret.into()).expect("valid key");
    let (signature, recovery_id) = signing_key
        .sign_prehash_recoverable(hash)
        .expect("signing failed");
    let sig_bytes: [u8; 64] = signature.to_bytes().into();
    (sig_bytes, recovery_id.to_byte() as u32)
}

// ──────────────────────────────────────────────────────────────────────
// VAA construction helpers
// ──────────────────────────────────────────────────────────────────────

fn build_body(
    timestamp: u32,
    nonce: u32,
    emitter_chain: u16,
    emitter_address: &[u8; 32],
    sequence: u64,
    consistency_level: u8,
    payload: &[u8],
) -> alloc::vec::Vec<u8> {
    let mut body = alloc::vec::Vec::new();
    body.extend_from_slice(&timestamp.to_be_bytes());
    body.extend_from_slice(&nonce.to_be_bytes());
    body.extend_from_slice(&emitter_chain.to_be_bytes());
    body.extend_from_slice(emitter_address);
    body.extend_from_slice(&sequence.to_be_bytes());
    body.push(consistency_level);
    body.extend_from_slice(payload);
    body
}

fn build_signed_vaa(
    guardian_set_index: u32,
    signers: &[(u8, [u8; 32])],
    body: &[u8],
) -> alloc::vec::Vec<u8> {
    let body_hash = keccak256(body);
    let double_hash = keccak256(&body_hash);

    let mut vaa = alloc::vec::Vec::new();
    vaa.push(1u8); // version
    vaa.extend_from_slice(&guardian_set_index.to_be_bytes());
    vaa.push(signers.len() as u8);

    for (guardian_index, secret) in signers {
        let (sig, recovery_id) = sign_hash(secret, &double_hash);
        vaa.push(*guardian_index);
        vaa.extend_from_slice(&sig);
        vaa.push(recovery_id as u8);
    }

    vaa.extend_from_slice(body);
    vaa
}

// ──────────────────────────────────────────────────────────────────────
// PTGM construction helpers
// ──────────────────────────────────────────────────────────────────────

const PTGM_MAGIC: [u8; 4] = [0x50, 0x54, 0x47, 0x4d]; // "PTGM"

fn build_ptgm_update_signer(
    chain_id: u16,
    executor_contract: &Address,
    target_contract: &Address,
    pubkey: &[u8; 33],
    expires_at: u64,
) -> alloc::vec::Vec<u8> {
    let mut data = alloc::vec::Vec::new();
    data.extend_from_slice(&PTGM_MAGIC);
    data.push(3); // module = Lazer
    data.push(1); // action = update_trusted_signer
    data.extend_from_slice(&chain_id.to_be_bytes());
    let executor = address_to_payload_bytes(executor_contract);
    data.push(executor.len() as u8);
    data.extend_from_slice(&executor);
    let target = address_to_payload_bytes(target_contract);
    data.push(target.len() as u8);
    data.extend_from_slice(&target);
    data.extend_from_slice(pubkey);
    data.extend_from_slice(&expires_at.to_be_bytes());
    data
}

fn build_ptgm_upgrade(
    chain_id: u16,
    executor_contract: &Address,
    target_contract: &Address,
    wasm_digest: &[u8; 32],
) -> alloc::vec::Vec<u8> {
    let mut data = alloc::vec::Vec::new();
    data.extend_from_slice(&PTGM_MAGIC);
    data.push(3); // module = Lazer
    data.push(0); // action = upgrade
    data.extend_from_slice(&chain_id.to_be_bytes());
    let executor = address_to_payload_bytes(executor_contract);
    data.push(executor.len() as u8);
    data.extend_from_slice(&executor);
    let target = address_to_payload_bytes(target_contract);
    data.push(target.len() as u8);
    data.extend_from_slice(&target);
    data.extend_from_slice(wasm_digest);
    data
}

fn build_ptgm_upgrade_executor(
    chain_id: u16,
    executor_contract: &Address,
    target_contract: &Address,
    wasm_digest: &[u8; 32],
) -> alloc::vec::Vec<u8> {
    let mut data = alloc::vec::Vec::new();
    data.extend_from_slice(&PTGM_MAGIC);
    data.push(3); // module = Lazer
    data.push(2); // action = upgrade_executor
    data.extend_from_slice(&chain_id.to_be_bytes());
    let executor = address_to_payload_bytes(executor_contract);
    data.push(executor.len() as u8);
    data.extend_from_slice(&executor);
    let target = address_to_payload_bytes(target_contract);
    data.push(target.len() as u8);
    data.extend_from_slice(&target);
    data.extend_from_slice(wasm_digest);
    data
}

fn address_to_payload_bytes(address: &Address) -> alloc::vec::Vec<u8> {
    let strkey = address.to_string();
    let mut out = alloc::vec![0u8; strkey.len() as usize];
    strkey.copy_into_slice(&mut out);
    out
}

fn build_guardian_set_upgrade_payload(
    target_chain: u16,
    new_index: u32,
    new_guardians: &[[u8; 20]],
) -> alloc::vec::Vec<u8> {
    let mut payload = alloc::vec::Vec::new();
    payload.extend_from_slice(b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00Core");
    payload.push(2); // action: guardian set upgrade
    payload.extend_from_slice(&target_chain.to_be_bytes());
    payload.extend_from_slice(&new_index.to_be_bytes());
    payload.push(new_guardians.len() as u8);
    for addr in new_guardians {
        payload.extend_from_slice(addr);
    }
    payload
}

// ──────────────────────────────────────────────────────────────────────
// Test environment setup
// ──────────────────────────────────────────────────────────────────────

const CHAIN_ID: u32 = 30;
const OWNER_EMITTER_CHAIN: u32 = 1; // Solana
/// Wormhole-core (guardian set upgrade) emitter chain. In production this is
/// Solana (chain 1) for the Wormhole core bridge governance emitter, distinct
/// from the Pyth governance emitter on the same chain.
const GS_UPGRADE_EMITTER_CHAIN: u32 = 1;

/// Shared test vector: trusted signer compressed public key (from Sui test suite).
fn test_trusted_signer_pubkey() -> [u8; 33] {
    hex_literal::hex!("03a4380f01136eb2640f90c17e1e319e02bbafbeef2e6e67dc48af53f9827e155b")
}

/// Shared test vector: signed Lazer update bytes.
fn test_lazer_update_bytes(env: &Env) -> Bytes {
    Bytes::from_slice(
        env,
        &hex_literal::hex!(
            "e4bd474d73a7e70a8e2b8de236b55dcc6a771b4a8a1533fe"
            "492f424fae162369fa14103e04c1c93302cef8a052110a95"
            "0da031f9dc5eade9e6099e95668aff2592ec1f7900fe0075"
            "d3c7934067e9c7f14a06000303010000000b00e1637ad535"
            "060000015a2507d335060000027f8bfdf53506000004f8ff"
            "0600070008000900000a601299cd3e0600000bc07595c73e"
            "0600000c014067e9c7f14a0600020000000b00971b209c2d"
            "0000000144056b9b2d0000000298fb6b9c2d00000004f8ff"
            "0600070008000900000a284444f92d0000000b480c07f92d"
            "0000000c014067e9c7f14a0600700000000b0020d85dd2d7"
            "8df30001000000000000000002000000000000000004f4ff"
            "060130f80bfeffffffff0701b8ab7057ec4a060008010020"
            "9db4060000000900000a00000000000000000b0000000000"
            "0000000c014067e9c7f14a0600"
        ),
    )
}

struct TestEnv<'a> {
    env: Env,
    executor_client: WormholeExecutorClient<'a>,
    lazer_client: PythLazerContractClient<'a>,
    guardian_secrets: alloc::vec::Vec<[u8; 32]>,
    owner_emitter_address: [u8; 32],
    gs_upgrade_emitter_address: [u8; 32],
}

/// Deploy both contracts with constructor args and a shared guardian set.
fn setup(num_guardians: u8) -> TestEnv<'static> {
    let env = Env::default();

    // Build guardian set.
    let mut secrets = alloc::vec::Vec::new();
    let mut guardian_addrs: Vec<BytesN<20>> = Vec::new(&env);
    for i in 0..num_guardians {
        let secret = test_secret(i);
        let addr = eth_address_from_secret(&secret);
        secrets.push(secret);
        guardian_addrs.push_back(BytesN::from_array(&env, &addr));
    }

    // Two distinct emitter addresses: governance owner vs. Wormhole-core
    // guardian-set-upgrade emitter.
    let owner_emitter_address = [0x42u8; 32];
    let gs_upgrade_emitter_address = [0x77u8; 32];
    assert_ne!(owner_emitter_address, gs_upgrade_emitter_address);

    // Deploy executor contract.
    let executor_id = env.register(
        WormholeExecutor,
        (
            CHAIN_ID,
            OWNER_EMITTER_CHAIN,
            BytesN::from_array(&env, &owner_emitter_address),
            GS_UPGRADE_EMITTER_CHAIN,
            BytesN::from_array(&env, &gs_upgrade_emitter_address),
            guardian_addrs,
            0u32,
        ),
    );
    let executor_client = WormholeExecutorClient::new(&env, &executor_id);

    // Deploy Lazer contract, initialized with executor as its governance authority.
    let lazer_id = env.register(
        PythLazerContract,
        (executor_id.clone(), None::<BytesN<33>>, None::<u64>),
    );
    let lazer_client = PythLazerContractClient::new(&env, &lazer_id);

    TestEnv {
        env,
        executor_client,
        lazer_client,
        guardian_secrets: secrets,
        owner_emitter_address,
        gs_upgrade_emitter_address,
    }
}

/// Build a governance VAA signed by the test guardian set.
fn build_governance_vaa(te: &TestEnv, sequence: u64, ptgm_payload: &[u8]) -> alloc::vec::Vec<u8> {
    let body = build_body(
        1000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        sequence,
        0,
        ptgm_payload,
    );
    let signers: alloc::vec::Vec<(u8, [u8; 32])> = te
        .guardian_secrets
        .iter()
        .enumerate()
        .map(|(i, s)| (i as u8, *s))
        .collect();
    build_signed_vaa(0, &signers, &body)
}

// ══════════════════════════════════════════════════════════════════════
// Integration tests
// ══════════════════════════════════════════════════════════════════════

// ──────────────────────────────────────────────────────────────────────
// Full governance flow: VAA -> executor -> Lazer contract
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_full_governance_add_trusted_signer() {
    let te = setup(1);

    let pubkey = test_trusted_signer_pubkey();
    let expires_at = 2_000_000_000u64;
    let update = test_lazer_update_bytes(&te.env);

    // Before governance, signer is not trusted and verification must fail.
    let pre_result = te.lazer_client.try_verify_update(&update);
    assert_eq!(pre_result.err().unwrap(), Ok(LazerError::SignerNotTrusted));

    // Build PTGM to add a trusted signer.
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        expires_at,
    );
    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    // Execute governance action: executor verifies VAA and dispatches to Lazer.
    te.executor_client.execute_governance_action(&vaa_bytes);

    // Verify the signer was added by submitting a signed Lazer update.
    let payload = te.lazer_client.verify_update(&update);
    assert!(!payload.is_empty());
}

#[test]
fn test_full_governance_update_signer_expiry() {
    let te = setup(1);

    let pubkey = test_trusted_signer_pubkey();

    // Add signer with short expiry.
    let ptgm1 = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        1_000,
    );
    let vaa1 = build_governance_vaa(&te, 1, &ptgm1);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &vaa1));

    // Update signer with longer expiry via a second governance action.
    let ptgm2 = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let vaa2 = build_governance_vaa(&te, 2, &ptgm2);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &vaa2));

    // Set ledger past the old expiry but before the new one.
    te.env.ledger().with_mut(|li| {
        li.timestamp = 5_000;
    });

    // Verification should succeed with the updated expiry.
    let update = test_lazer_update_bytes(&te.env);
    let payload = te.lazer_client.verify_update(&update);
    assert!(!payload.is_empty());
}

#[test]
fn test_full_governance_remove_signer() {
    let te = setup(1);

    let pubkey = test_trusted_signer_pubkey();

    // Add signer.
    let ptgm1 = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let vaa1 = build_governance_vaa(&te, 1, &ptgm1);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &vaa1));

    // Verify signer works.
    let update = test_lazer_update_bytes(&te.env);
    assert!(!te.lazer_client.verify_update(&update).is_empty());

    // Remove signer via governance (expires_at = 0).
    let ptgm2 = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        0,
    );
    let vaa2 = build_governance_vaa(&te, 2, &ptgm2);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &vaa2));

    // Verification should now fail.
    let result = te.lazer_client.try_verify_update(&update);
    assert_eq!(result.err().unwrap(), Ok(LazerError::SignerNotTrusted));
}

// ──────────────────────────────────────────────────────────────────────
// Full verification flow: governance -> verify -> parse payload
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_full_verification_and_payload_parsing() {
    let te = setup(1);

    let pubkey = test_trusted_signer_pubkey();

    // Add trusted signer via governance.
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &vaa_raw));

    // Submit a signed Lazer update and get verified payload.
    let update = test_lazer_update_bytes(&te.env);
    let verified_payload = te.lazer_client.verify_update(&update);

    // Parse the verified payload off-chain.
    let parsed = payload::parse_payload(&verified_payload).unwrap();

    assert_eq!(parsed.timestamp, 1_771_252_161_800_000);
    assert_eq!(parsed.channel, payload::Channel::FixedRate200ms);
    assert_eq!(parsed.feeds.len(), 3);

    // Feed 1: BTC/USD
    let btc = &parsed.feeds[0];
    assert_eq!(btc.feed_id, 1);
    assert_eq!(btc.price, Some(6_828_284_601_313));
    assert_eq!(btc.exponent, Some(-8));
    assert_eq!(btc.market_session, Some(payload::MarketSession::Regular));

    // Feed 2: ETH/USD
    let eth = &parsed.feeds[1];
    assert_eq!(eth.feed_id, 2);
    assert_eq!(eth.price, Some(195_892_878_231));
    assert_eq!(eth.exponent, Some(-8));

    // Feed 3: SOL/USD (with funding rate)
    let sol = &parsed.feeds[2];
    assert_eq!(sol.feed_id, 112);
    assert_eq!(sol.price, Some(68_554_377_427_540_000));
    assert_eq!(sol.exponent, Some(-12));
    assert_eq!(sol.funding_rate, Some(-32_770_000));
    assert_eq!(sol.funding_timestamp, Some(1_771_228_800_003_000));
}

// ──────────────────────────────────────────────────────────────────────
// Upgrade governance flow
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_governance_upgrade_dispatched_to_lazer() {
    let te = setup(1);

    // Upload a real wasm blob so the dispatched upgrade can succeed.
    let wasm_hash = te.env.deployer().upload_contract_wasm(Bytes::new(&te.env));
    let wasm_digest: [u8; 32] = wasm_hash.into();
    let ptgm = build_ptgm_upgrade(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &wasm_digest,
    );
    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    let result = te.executor_client.try_execute_governance_action(&vaa_bytes);
    assert!(result.is_ok());
}

// ──────────────────────────────────────────────────────────────────────
// Guardian set upgrade flow
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_guardian_set_upgrade_then_governance() {
    let te = setup(1);

    // Upgrade guardian set from index 0 to index 1 with new guardians.
    let new_secret = test_secret(10);
    let new_addr = eth_address_from_secret(&new_secret);
    let upgrade_payload = build_guardian_set_upgrade_payload(0, 1, &[new_addr]);
    let body = build_body(
        1000,
        0,
        GS_UPGRADE_EMITTER_CHAIN as u16,
        &te.gs_upgrade_emitter_address,
        1,
        0,
        &upgrade_payload,
    );
    let vaa_raw = build_signed_vaa(0, &[(0u8, te.guardian_secrets[0])], &body);
    te.executor_client
        .update_guardian_set(&Bytes::from_slice(&te.env, &vaa_raw));

    // Now use the new guardian set to execute a governance action.
    let pubkey = test_trusted_signer_pubkey();
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let gov_body = build_body(
        2000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        2,
        0,
        &ptgm,
    );
    let gov_vaa = build_signed_vaa(1, &[(0u8, new_secret)], &gov_body);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &gov_vaa));

    // Verify the signer was added.
    let update = test_lazer_update_bytes(&te.env);
    let payload = te.lazer_client.verify_update(&update);
    assert!(!payload.is_empty());
}

#[test]
fn test_sequential_guardian_upgrades_then_governance() {
    let te = setup(1);

    // Upgrade 0 -> 1
    let secret_1 = test_secret(10);
    let addr_1 = eth_address_from_secret(&secret_1);
    let up1 = build_guardian_set_upgrade_payload(0, 1, &[addr_1]);
    let body1 = build_body(
        1000,
        0,
        GS_UPGRADE_EMITTER_CHAIN as u16,
        &te.gs_upgrade_emitter_address,
        1,
        0,
        &up1,
    );
    let vaa1 = build_signed_vaa(0, &[(0u8, te.guardian_secrets[0])], &body1);
    te.executor_client
        .update_guardian_set(&Bytes::from_slice(&te.env, &vaa1));

    // Upgrade 1 -> 2
    let secret_2a = test_secret(20);
    let secret_2b = test_secret(21);
    let addr_2a = eth_address_from_secret(&secret_2a);
    let addr_2b = eth_address_from_secret(&secret_2b);
    let up2 = build_guardian_set_upgrade_payload(0, 2, &[addr_2a, addr_2b]);
    let body2 = build_body(
        2000,
        0,
        GS_UPGRADE_EMITTER_CHAIN as u16,
        &te.gs_upgrade_emitter_address,
        2,
        0,
        &up2,
    );
    let vaa2 = build_signed_vaa(1, &[(0u8, secret_1)], &body2);
    te.executor_client
        .update_guardian_set(&Bytes::from_slice(&te.env, &vaa2));

    // Execute governance with the new 2-guardian set.
    let pubkey = test_trusted_signer_pubkey();
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let gov_body = build_body(
        3000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        3,
        0,
        &ptgm,
    );
    let gov_vaa = build_signed_vaa(2, &[(0u8, secret_2a), (1u8, secret_2b)], &gov_body);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &gov_vaa));

    // Verify signer was added.
    let update = test_lazer_update_bytes(&te.env);
    assert!(!te.lazer_client.verify_update(&update).is_empty());
}

// ──────────────────────────────────────────────────────────────────────
// Emitter-split tests: guardian-set-upgrade emitter is independent of
// the owner (governance) emitter. Cross-emitter use must be rejected.
// ──────────────────────────────────────────────────────────────────────

/// A guardian-signed Core/action=2 VAA using the *owner* emitter must be
/// rejected by `update_guardian_set` — guardian set upgrades may only come
/// from the configured guardian-set-upgrade emitter.
#[test]
fn test_guardian_set_upgrade_from_owner_emitter_rejected() {
    let te = setup(1);

    let new_addr = eth_address_from_secret(&test_secret(10));
    let upgrade_payload = build_guardian_set_upgrade_payload(0, 1, &[new_addr]);
    let body = build_body(
        1000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        1,
        0,
        &upgrade_payload,
    );
    let vaa_raw = build_signed_vaa(0, &[(0u8, te.guardian_secrets[0])], &body);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    let result = te.executor_client.try_update_guardian_set(&vaa_bytes);
    assert!(result.is_err());
}

/// A guardian-signed governance VAA from the *guardian-set-upgrade* emitter
/// must be rejected by `execute_governance_action` — only the configured
/// owner emitter may issue PTGM governance actions.
#[test]
fn test_governance_from_gs_upgrade_emitter_rejected() {
    let te = setup(1);

    let pubkey = test_trusted_signer_pubkey();
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let body = build_body(
        1000,
        0,
        GS_UPGRADE_EMITTER_CHAIN as u16,
        &te.gs_upgrade_emitter_address,
        1,
        0,
        &ptgm,
    );
    let vaa_raw = build_signed_vaa(0, &[(0u8, te.guardian_secrets[0])], &body);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    let result = te.executor_client.try_execute_governance_action(&vaa_bytes);
    assert!(result.is_err());
}

// ──────────────────────────────────────────────────────────────────────
// Negative tests: expired signer
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_expired_signer_after_governance() {
    let te = setup(1);

    let pubkey = test_trusted_signer_pubkey();

    // Add signer with short expiry via governance.
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        1_000,
    );
    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &vaa_raw));

    // Set ledger past expiry.
    te.env.ledger().with_mut(|li| {
        li.timestamp = 1_000;
    });

    let update = test_lazer_update_bytes(&te.env);
    let result = te.lazer_client.try_verify_update(&update);
    assert_eq!(result.err().unwrap(), Ok(LazerError::SignerExpired));
}

// ──────────────────────────────────────────────────────────────────────
// Negative tests: wrong emitter
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_governance_wrong_emitter_chain() {
    let te = setup(1);

    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &[0xAA; 33],
        1000,
    );
    // Use emitter_chain = 2 instead of 1.
    let body = build_body(1000, 0, 2, &te.owner_emitter_address, 1, 0, &ptgm);
    let vaa_raw = build_signed_vaa(0, &[(0u8, te.guardian_secrets[0])], &body);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    let result = te.executor_client.try_execute_governance_action(&vaa_bytes);
    assert!(result.is_err());
}

#[test]
fn test_governance_wrong_emitter_address() {
    let te = setup(1);

    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &[0xAA; 33],
        1000,
    );
    let wrong_address = [0x99u8; 32];
    let body = build_body(
        1000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &wrong_address,
        1,
        0,
        &ptgm,
    );
    let vaa_raw = build_signed_vaa(0, &[(0u8, te.guardian_secrets[0])], &body);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    let result = te.executor_client.try_execute_governance_action(&vaa_bytes);
    assert!(result.is_err());
}

#[test]
fn test_governance_wrong_executor_address_in_payload() {
    let te = setup(1);

    let wrong_executor = Address::generate(&te.env);
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &wrong_executor,
        &te.lazer_client.address,
        &[0xAA; 33],
        1000,
    );
    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    let result = te.executor_client.try_execute_governance_action(&vaa_bytes);
    assert!(result.is_err());
}

// ──────────────────────────────────────────────────────────────────────
// Negative tests: replayed VAA
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_governance_replayed_vaa() {
    let te = setup(1);

    let pubkey = test_trusted_signer_pubkey();
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    // First execution succeeds.
    te.executor_client.execute_governance_action(&vaa_bytes);

    // Replay with the same sequence should fail.
    let result = te.executor_client.try_execute_governance_action(&vaa_bytes);
    assert!(result.is_err());

    // Earlier sequence (never executed, but now below last_executed) should fail.
    let ptgm_earlier = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        3_000_000_000,
    );
    let vaa_earlier = build_governance_vaa(&te, 0, &ptgm_earlier);
    let result = te
        .executor_client
        .try_execute_governance_action(&Bytes::from_slice(&te.env, &vaa_earlier));
    assert!(result.is_err());

    // Next sequence succeeds.
    let ptgm_next = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        3_000_000_000,
    );
    let vaa_next = build_governance_vaa(&te, 2, &ptgm_next);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &vaa_next));
}

#[test]
fn test_governance_stale_sequence() {
    let te = setup(1);

    let pubkey = test_trusted_signer_pubkey();

    // Execute sequence 5.
    let ptgm1 = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let vaa1 = build_governance_vaa(&te, 5, &ptgm1);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &vaa1));

    // Try sequence 3 (stale).
    let ptgm2 = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        3_000_000_000,
    );
    let vaa2 = build_governance_vaa(&te, 3, &ptgm2);
    let result = te
        .executor_client
        .try_execute_governance_action(&Bytes::from_slice(&te.env, &vaa2));
    assert!(result.is_err());
}

// ──────────────────────────────────────────────────────────────────────
// Negative tests: unauthorized governance call
// ──────────────────────────────────────────────────────────────────────

#[test]
#[should_panic(expected = "HostError: Error(Auth")]
fn test_unauthorized_direct_signer_update() {
    let test_env = setup(1);

    let pubkey = BytesN::from_array(&test_env.env, &test_trusted_signer_pubkey());

    // Direct invocation without executor authorization must fail.
    test_env
        .lazer_client
        .update_trusted_signer(&pubkey, &2_000_000_000u64);
}

#[test]
#[should_panic(expected = "HostError: Error(Auth")]
fn test_unauthorized_direct_upgrade() {
    let test_env = setup(1);

    let fake_hash = BytesN::from_array(&test_env.env, &[0u8; 32]);

    // Direct invocation without executor authorization must fail.
    test_env.lazer_client.upgrade(&fake_hash);
}

// ──────────────────────────────────────────────────────────────────────
// Negative tests: invalid PTGM in governance VAA
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_governance_invalid_ptgm_magic() {
    let te = setup(1);

    let mut ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &[0xAA; 33],
        1000,
    );
    ptgm[0] = 0xFF; // corrupt PTGM magic

    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    let result = te
        .executor_client
        .try_execute_governance_action(&Bytes::from_slice(&te.env, &vaa_raw));
    assert!(result.is_err());
}

#[test]
fn test_governance_wrong_target_chain() {
    let te = setup(1);

    // PTGM targets chain 99, but executor is on chain 30.
    let ptgm = build_ptgm_update_signer(
        99,
        &te.executor_client.address,
        &te.lazer_client.address,
        &[0xAA; 33],
        1000,
    );
    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    let result = te
        .executor_client
        .try_execute_governance_action(&Bytes::from_slice(&te.env, &vaa_raw));
    assert!(result.is_err());
}

// ──────────────────────────────────────────────────────────────────────
// Multi-guardian quorum test
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_governance_with_quorum() {
    // 3 guardians, quorum = 3.
    let te = setup(3);

    let pubkey = test_trusted_signer_pubkey();
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );

    // All 3 guardians sign.
    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &vaa_raw));

    let update = test_lazer_update_bytes(&te.env);
    assert!(!te.lazer_client.verify_update(&update).is_empty());
}

// ──────────────────────────────────────────────────────────────────────
// Verify update without any trusted signer
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_verify_update_no_signers_configured() {
    let te = setup(1);

    // No governance action to add signers — verify should fail.
    let update = test_lazer_update_bytes(&te.env);
    let result = te.lazer_client.try_verify_update(&update);
    assert_eq!(result.err().unwrap(), Ok(LazerError::SignerNotTrusted));
}

// ──────────────────────────────────────────────────────────────────────
// Executor self-upgrade tests
// ──────────────────────────────────────────────────────────────────────

/// Happy path: owner sends an upgrade-executor governance VAA and the
/// executor upgrades itself.
#[test]
fn test_upgrade_executor_happy_path() {
    let te = setup(1);

    // Upload a wasm blob so update_current_contract_wasm succeeds.
    let wasm_hash = te.env.deployer().upload_contract_wasm(Bytes::new(&te.env));
    let wasm_digest: [u8; 32] = wasm_hash.into();

    let ptgm = build_ptgm_upgrade_executor(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.executor_client.address, // target = executor (self-upgrade)
        &wasm_digest,
    );
    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    let result = te.executor_client.try_execute_governance_action(&vaa_bytes);
    assert!(result.is_ok());
}

/// Adversarial: an upgrade-executor VAA from the guardian-set-upgrade
/// emitter must be rejected — only the owner emitter may issue PTGM
/// governance actions (including self-upgrades).
#[test]
fn test_upgrade_executor_from_gs_upgrade_emitter_rejected() {
    let te = setup(1);

    let wasm_hash = te.env.deployer().upload_contract_wasm(Bytes::new(&te.env));
    let wasm_digest: [u8; 32] = wasm_hash.into();

    let ptgm = build_ptgm_upgrade_executor(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.executor_client.address,
        &wasm_digest,
    );
    // Use the guardian-set-upgrade emitter instead of the owner emitter.
    let body = build_body(
        1000,
        0,
        GS_UPGRADE_EMITTER_CHAIN as u16,
        &te.gs_upgrade_emitter_address,
        1,
        0,
        &ptgm,
    );
    let signers: alloc::vec::Vec<(u8, [u8; 32])> = te
        .guardian_secrets
        .iter()
        .enumerate()
        .map(|(i, s)| (i as u8, *s))
        .collect();
    let vaa_raw = build_signed_vaa(0, &signers, &body);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    let result = te.executor_client.try_execute_governance_action(&vaa_bytes);
    assert!(result.is_err());
}

/// Adversarial: an upgrade-executor VAA targeting a different contract
/// (not the executor itself) must be rejected.
#[test]
fn test_upgrade_executor_wrong_target_rejected() {
    let te = setup(1);

    let wasm_hash = te.env.deployer().upload_contract_wasm(Bytes::new(&te.env));
    let wasm_digest: [u8; 32] = wasm_hash.into();

    // Target is the lazer contract, not the executor itself.
    let ptgm = build_ptgm_upgrade_executor(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &wasm_digest,
    );
    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    let result = te.executor_client.try_execute_governance_action(&vaa_bytes);
    assert!(result.is_err());
}

/// Adversarial: replay protection applies to upgrade-executor VAAs.
#[test]
fn test_upgrade_executor_replay_rejected() {
    let te = setup(1);

    let wasm_hash = te.env.deployer().upload_contract_wasm(Bytes::new(&te.env));
    let wasm_digest: [u8; 32] = wasm_hash.into();

    let ptgm = build_ptgm_upgrade_executor(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.executor_client.address,
        &wasm_digest,
    );
    let vaa_raw = build_governance_vaa(&te, 1, &ptgm);
    let vaa_bytes = Bytes::from_slice(&te.env, &vaa_raw);

    // First execution succeeds.
    te.executor_client.execute_governance_action(&vaa_bytes);

    // Replay with the same sequence must fail.
    let result = te.executor_client.try_execute_governance_action(&vaa_bytes);
    assert!(result.is_err());
}

// ──────────────────────────────────────────────────────────────────────
// Guardian set 24-hour expiration window tests
// ──────────────────────────────────────────────────────────────────────

/// Helper: upgrade guardian set and return the new secret.
fn do_guardian_set_upgrade(
    te: &TestEnv,
    old_guardian_set_index: u32,
    new_index: u32,
    old_signers: &[(u8, [u8; 32])],
    new_guardians: &[[u8; 20]],
) {
    let upgrade_payload = build_guardian_set_upgrade_payload(0, new_index, new_guardians);
    let body = build_body(
        1000 + new_index,
        0,
        GS_UPGRADE_EMITTER_CHAIN as u16,
        &te.gs_upgrade_emitter_address,
        new_index as u64,
        0,
        &upgrade_payload,
    );
    let vaa_raw = build_signed_vaa(old_guardian_set_index, old_signers, &body);
    te.executor_client
        .update_guardian_set(&Bytes::from_slice(&te.env, &vaa_raw));
}

/// After a guardian set upgrade, VAAs signed by the old guardian set should
/// still be accepted during the 24-hour grace window.
#[test]
fn test_old_guardian_set_accepted_during_24h_window() {
    let te = setup(1);

    // Set an initial timestamp.
    te.env.ledger().with_mut(|li| {
        li.timestamp = 100_000;
    });

    // Upgrade guardian set 0 -> 1.
    let new_secret = test_secret(10);
    let new_addr = eth_address_from_secret(&new_secret);
    do_guardian_set_upgrade(&te, 0, 1, &[(0u8, te.guardian_secrets[0])], &[new_addr]);

    // Add a trusted signer via governance using the NEW guardian set (index 1).
    let pubkey = test_trusted_signer_pubkey();
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let gov_body = build_body(
        2000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        2,
        0,
        &ptgm,
    );
    let gov_vaa = build_signed_vaa(1, &[(0u8, new_secret)], &gov_body);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &gov_vaa));

    // Now use the OLD guardian set (index 0) to execute another governance action
    // within the 24h window. Advance time by 12 hours (< 24h).
    te.env.ledger().with_mut(|li| {
        li.timestamp = 100_000 + 43_200; // 12 hours later
    });

    let ptgm2 = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        3_000_000_000,
    );
    let gov_body2 = build_body(
        3000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        3,
        0,
        &ptgm2,
    );
    // Sign with the OLD guardian set (index 0).
    let gov_vaa2 = build_signed_vaa(0, &[(0u8, te.guardian_secrets[0])], &gov_body2);
    let result = te
        .executor_client
        .try_execute_governance_action(&Bytes::from_slice(&te.env, &gov_vaa2));
    assert!(
        result.is_ok(),
        "Old guardian set should be accepted within 24h window"
    );
}

/// After the 24-hour grace window expires, VAAs signed by the old guardian set
/// should be rejected.
#[test]
fn test_old_guardian_set_rejected_after_24h_window() {
    let te = setup(1);

    te.env.ledger().with_mut(|li| {
        li.timestamp = 100_000;
    });

    // Upgrade guardian set 0 -> 1.
    let new_secret = test_secret(10);
    let new_addr = eth_address_from_secret(&new_secret);
    do_guardian_set_upgrade(&te, 0, 1, &[(0u8, te.guardian_secrets[0])], &[new_addr]);

    // Advance time past 24 hours.
    te.env.ledger().with_mut(|li| {
        li.timestamp = 100_000 + 86_400; // exactly 24h (expiration is >=)
    });

    // Try governance with the OLD guardian set (index 0) — should fail.
    let pubkey = test_trusted_signer_pubkey();
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let gov_body = build_body(
        2000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        1,
        0,
        &ptgm,
    );
    let gov_vaa = build_signed_vaa(0, &[(0u8, te.guardian_secrets[0])], &gov_body);
    let result = te
        .executor_client
        .try_execute_governance_action(&Bytes::from_slice(&te.env, &gov_vaa));
    assert_eq!(
        result.err().unwrap().unwrap(),
        ExecutorError::GuardianSetExpired
    );
}

/// Multiple sequential upgrades: each old set gets its own 24h window.
/// The most recently retired set should be valid, while an older one
/// that has timed out should be rejected.
#[test]
fn test_multiple_sequential_upgrades_with_overlapping_windows() {
    let te = setup(1);

    te.env.ledger().with_mut(|li| {
        li.timestamp = 100_000;
    });

    // Upgrade 0 -> 1
    let secret_1 = test_secret(10);
    let addr_1 = eth_address_from_secret(&secret_1);
    do_guardian_set_upgrade(&te, 0, 1, &[(0u8, te.guardian_secrets[0])], &[addr_1]);

    // Advance 12 hours, then upgrade 1 -> 2
    te.env.ledger().with_mut(|li| {
        li.timestamp = 100_000 + 43_200; // 12h after first upgrade
    });

    let secret_2 = test_secret(20);
    let addr_2 = eth_address_from_secret(&secret_2);
    do_guardian_set_upgrade(&te, 1, 2, &[(0u8, secret_1)], &[addr_2]);

    // First, add a trusted signer using current set (index 2) so we can test governance.
    let pubkey = test_trusted_signer_pubkey();
    let ptgm_setup = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let gov_body_setup = build_body(
        3000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        3,
        0,
        &ptgm_setup,
    );
    let gov_vaa_setup = build_signed_vaa(2, &[(0u8, secret_2)], &gov_body_setup);
    te.executor_client
        .execute_governance_action(&Bytes::from_slice(&te.env, &gov_vaa_setup));

    // Now at T=143200: set 0 expires at T=186400, set 1 expires at T=229600.
    // Both old sets should still be valid.

    // Advance to T=190000 (set 0 has expired, set 1 still valid).
    te.env.ledger().with_mut(|li| {
        li.timestamp = 190_000;
    });

    // Set 1 (expired at T=229600) should still work.
    let ptgm_1 = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        3_000_000_000,
    );
    let gov_body_1 = build_body(
        4000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        4,
        0,
        &ptgm_1,
    );
    let gov_vaa_1 = build_signed_vaa(1, &[(0u8, secret_1)], &gov_body_1);
    let result = te
        .executor_client
        .try_execute_governance_action(&Bytes::from_slice(&te.env, &gov_vaa_1));
    assert!(result.is_ok(), "Set 1 should still be valid at T=190000");

    // Set 0 (expired at T=186400) should be rejected.
    let ptgm_0 = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        4_000_000_000,
    );
    let gov_body_0 = build_body(
        5000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        5,
        0,
        &ptgm_0,
    );
    let gov_vaa_0 = build_signed_vaa(0, &[(0u8, te.guardian_secrets[0])], &gov_body_0);
    let result = te
        .executor_client
        .try_execute_governance_action(&Bytes::from_slice(&te.env, &gov_vaa_0));
    assert_eq!(
        result.err().unwrap().unwrap(),
        ExecutorError::GuardianSetExpired
    );
}

/// Adversarial: replaying a VAA with a very old guardian set index that was
/// never stored should fail with GuardianSetNotFound.
#[test]
fn test_replay_with_unknown_guardian_set_index() {
    let te = setup(1);

    // Try to verify a VAA with guardian_set_index = 99 (never existed).
    let pubkey = test_trusted_signer_pubkey();
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let body = build_body(
        1000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        1,
        0,
        &ptgm,
    );
    // Sign with a valid secret but claim guardian_set_index = 99.
    let vaa_raw = build_signed_vaa(99, &[(0u8, te.guardian_secrets[0])], &body);
    let result = te
        .executor_client
        .try_execute_governance_action(&Bytes::from_slice(&te.env, &vaa_raw));
    assert_eq!(
        result.err().unwrap().unwrap(),
        ExecutorError::GuardianSetNotFound
    );
}

/// Current guardian set should always be accepted regardless of time elapsed.
#[test]
fn test_current_guardian_set_always_accepted() {
    let te = setup(1);

    te.env.ledger().with_mut(|li| {
        li.timestamp = 100_000;
    });

    // Upgrade guardian set 0 -> 1.
    let new_secret = test_secret(10);
    let new_addr = eth_address_from_secret(&new_secret);
    do_guardian_set_upgrade(&te, 0, 1, &[(0u8, te.guardian_secrets[0])], &[new_addr]);

    // Advance far into the future.
    te.env.ledger().with_mut(|li| {
        li.timestamp = 100_000_000;
    });

    // Current set (index 1) should always work.
    let pubkey = test_trusted_signer_pubkey();
    let ptgm = build_ptgm_update_signer(
        CHAIN_ID as u16,
        &te.executor_client.address,
        &te.lazer_client.address,
        &pubkey,
        2_000_000_000,
    );
    let gov_body = build_body(
        2000,
        0,
        OWNER_EMITTER_CHAIN as u16,
        &te.owner_emitter_address,
        2,
        0,
        &ptgm,
    );
    let gov_vaa = build_signed_vaa(1, &[(0u8, new_secret)], &gov_body);
    let result = te
        .executor_client
        .try_execute_governance_action(&Bytes::from_slice(&te.env, &gov_vaa));
    assert!(result.is_ok(), "Current guardian set should never expire");
}
