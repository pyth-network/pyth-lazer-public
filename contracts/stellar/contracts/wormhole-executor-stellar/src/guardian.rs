use soroban_sdk::{contracttype, BytesN, Env, Vec};

use crate::error::ContractError;

/// TTL threshold: extend when TTL drops below this (~6 days in ledgers at ~5s/ledger).
const TTL_THRESHOLD: u32 = 100_000;
/// TTL extension target: extend to this value (~29 days in ledgers at ~5s/ledger).
const TTL_EXTEND_TO: u32 = 500_000;

/// 24-hour expiration window in seconds for old guardian sets after an upgrade.
const GUARDIAN_SET_EXPIRY: u64 = 86400;

/// A guardian set stored with an optional expiration timestamp.
///
/// When `expiration_time` is `None`, the set never expires (it is the current set).
/// When `Some(t)`, the set is rejected once the ledger timestamp reaches `t`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct StoredGuardianSet {
    pub keys: Vec<BytesN<20>>,
    pub expiration_time: Option<u64>,
}

/// Storage keys for the executor contract.
#[contracttype]
#[derive(Clone, Debug)]
pub enum DataKey {
    /// Whether the contract has been initialized.
    Initialized,
    /// The current guardian set index (u32).
    GuardianSetIndex,
    /// The Wormhole chain ID for this chain (u16 stored as u32).
    ChainId,
    /// The authorized governance emitter chain ID (u16 stored as u32).
    OwnerEmitterChain,
    /// The authorized governance emitter address (32 bytes).
    OwnerEmitterAddress,
    /// The authorized guardian-set-upgrade emitter chain ID (u16 stored as u32).
    /// Distinct from the governance owner emitter — this emitter is only
    /// authorized to upgrade the guardian set, not to execute governance
    /// actions against target contracts.
    GsUpgradeEmitterChain,
    /// The authorized guardian-set-upgrade emitter address (32 bytes).
    GsUpgradeEmitterAddress,
    /// The last executed governance sequence number (u64).
    LastExecutedSequence,
    /// A guardian set stored by its index: StoredGuardianSet.
    GuardianSetByIndex(u32),
}

/// Store the initial contract configuration.
///
/// This must be called exactly once. It stores the guardian set, owner emitter
/// chain/address, guardian-set-upgrade emitter chain/address, and chain ID.
#[allow(clippy::too_many_arguments)]
pub fn initialize(
    env: &Env,
    chain_id: u32,
    owner_emitter_chain: u32,
    owner_emitter_address: BytesN<32>,
    gs_upgrade_emitter_chain: u32,
    gs_upgrade_emitter_address: BytesN<32>,
    initial_guardian_set: Vec<BytesN<20>>,
    guardian_set_index: u32,
) -> Result<(), ContractError> {
    if initial_guardian_set.is_empty() {
        return Err(ContractError::EmptyGuardianSet);
    }

    if env.storage().instance().has(&DataKey::Initialized) {
        return Err(ContractError::AlreadyInitialized);
    }

    env.storage().instance().set(&DataKey::Initialized, &true);
    let stored_set = StoredGuardianSet {
        keys: initial_guardian_set,
        expiration_time: None, // current set never expires
    };
    env.storage().persistent().set(
        &DataKey::GuardianSetByIndex(guardian_set_index),
        &stored_set,
    );
    env.storage()
        .persistent()
        .set(&DataKey::GuardianSetIndex, &guardian_set_index);
    env.storage().instance().set(&DataKey::ChainId, &chain_id);
    env.storage()
        .instance()
        .set(&DataKey::OwnerEmitterChain, &owner_emitter_chain);
    env.storage()
        .instance()
        .set(&DataKey::OwnerEmitterAddress, &owner_emitter_address);
    env.storage()
        .instance()
        .set(&DataKey::GsUpgradeEmitterChain, &gs_upgrade_emitter_chain);
    env.storage().instance().set(
        &DataKey::GsUpgradeEmitterAddress,
        &gs_upgrade_emitter_address,
    );
    env.storage()
        .persistent()
        .set(&DataKey::LastExecutedSequence, &0u64);

    // Extend TTL for all persistent entries.
    extend_guardian_set_by_index_ttl(env, guardian_set_index);
    extend_persistent_ttl(env);
    extend_instance_ttl(env);

    Ok(())
}

/// Check that the contract has been initialized.
pub fn require_initialized(env: &Env) -> Result<(), ContractError> {
    if !env.storage().instance().has(&DataKey::Initialized) {
        return Err(ContractError::NotInitialized);
    }
    Ok(())
}

/// Get the current guardian set keys.
pub fn get_guardian_set(env: &Env) -> Result<Vec<BytesN<20>>, ContractError> {
    let index = get_guardian_set_index(env)?;
    let stored = get_guardian_set_by_index(env, index)?;
    Ok(stored.keys)
}

/// Get a guardian set by its index.
pub fn get_guardian_set_by_index(
    env: &Env,
    index: u32,
) -> Result<StoredGuardianSet, ContractError> {
    extend_guardian_set_by_index_ttl(env, index);
    env.storage()
        .persistent()
        .get(&DataKey::GuardianSetByIndex(index))
        .ok_or(ContractError::GuardianSetNotFound)
}

/// Get the current guardian set index.
pub fn get_guardian_set_index(env: &Env) -> Result<u32, ContractError> {
    env.storage()
        .persistent()
        .get(&DataKey::GuardianSetIndex)
        .ok_or(ContractError::NotInitialized)
}

/// Get the chain ID.
pub fn get_chain_id(env: &Env) -> Result<u32, ContractError> {
    env.storage()
        .instance()
        .get(&DataKey::ChainId)
        .ok_or(ContractError::NotInitialized)
}

/// Get the owner emitter chain ID.
pub fn get_owner_emitter_chain(env: &Env) -> Result<u32, ContractError> {
    env.storage()
        .instance()
        .get(&DataKey::OwnerEmitterChain)
        .ok_or(ContractError::NotInitialized)
}

/// Get the owner emitter address.
pub fn get_owner_emitter_address(env: &Env) -> Result<BytesN<32>, ContractError> {
    env.storage()
        .instance()
        .get(&DataKey::OwnerEmitterAddress)
        .ok_or(ContractError::NotInitialized)
}

/// Get the guardian-set-upgrade emitter chain ID.
pub fn get_gs_upgrade_emitter_chain(env: &Env) -> Result<u32, ContractError> {
    env.storage()
        .instance()
        .get(&DataKey::GsUpgradeEmitterChain)
        .ok_or(ContractError::NotInitialized)
}

/// Get the guardian-set-upgrade emitter address.
pub fn get_gs_upgrade_emitter_address(env: &Env) -> Result<BytesN<32>, ContractError> {
    env.storage()
        .instance()
        .get(&DataKey::GsUpgradeEmitterAddress)
        .ok_or(ContractError::NotInitialized)
}

/// Get the last executed sequence number.
pub fn get_last_executed_sequence(env: &Env) -> u64 {
    env.storage()
        .persistent()
        .get(&DataKey::LastExecutedSequence)
        .unwrap_or(0u64)
}

/// Set the last executed sequence number.
pub fn set_last_executed_sequence(env: &Env, sequence: u64) {
    env.storage()
        .persistent()
        .set(&DataKey::LastExecutedSequence, &sequence);
}

/// Update the guardian set. Expires the old set (24h window) and stores the new one.
pub fn store_guardian_set(env: &Env, new_guardian_set: Vec<BytesN<20>>, new_index: u32) {
    // Expire the old guardian set with a 24-hour grace window.
    let old_index = new_index - 1;
    if let Some(mut old_set) = env
        .storage()
        .persistent()
        .get::<_, StoredGuardianSet>(&DataKey::GuardianSetByIndex(old_index))
    {
        old_set.expiration_time = Some(env.ledger().timestamp() + GUARDIAN_SET_EXPIRY);
        env.storage()
            .persistent()
            .set(&DataKey::GuardianSetByIndex(old_index), &old_set);
        extend_guardian_set_by_index_ttl(env, old_index);
    }

    // Store the new guardian set (None expiration means it never expires while current).
    let stored_set = StoredGuardianSet {
        keys: new_guardian_set,
        expiration_time: None,
    };
    env.storage()
        .persistent()
        .set(&DataKey::GuardianSetByIndex(new_index), &stored_set);
    env.storage()
        .persistent()
        .set(&DataKey::GuardianSetIndex, &new_index);
    extend_guardian_set_by_index_ttl(env, new_index);
    extend_persistent_ttl(env);
}

/// Derive an Ethereum address from an uncompressed secp256k1 public key.
///
/// The uncompressed key is 65 bytes: `0x04 || x (32) || y (32)`.
/// The Ethereum address is `keccak256(x || y)[12..32]`.
pub fn eth_address_from_pubkey(env: &Env, uncompressed: &BytesN<65>) -> BytesN<20> {
    // Get x || y (skip the 0x04 prefix byte).
    let raw_key = env.crypto().keccak256(&soroban_sdk::Bytes::from_slice(
        env,
        &uncompressed.to_array()[1..65],
    ));
    let hash_array = raw_key.to_array();
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&hash_array[12..32]);
    BytesN::from_array(env, &addr)
}

/// Compute the quorum threshold: 2/3 + 1 of the guardian set size.
///
/// Uses the same fixed-point formula as the EVM implementation:
/// `(((num_guardians * 10) / 3) * 2) / 10 + 1`
pub fn quorum(num_guardians: u32) -> u32 {
    (((num_guardians * 10) / 3) * 2) / 10 + 1
}

/// Extend TTL on a specific guardian set entry by index.
fn extend_guardian_set_by_index_ttl(env: &Env, index: u32) {
    if env
        .storage()
        .persistent()
        .has(&DataKey::GuardianSetByIndex(index))
    {
        env.storage().persistent().extend_ttl(
            &DataKey::GuardianSetByIndex(index),
            TTL_THRESHOLD,
            TTL_EXTEND_TO,
        );
    }
}

/// Extend TTL on non-guardian-set persistent storage entries.
fn extend_persistent_ttl(env: &Env) {
    env.storage()
        .persistent()
        .extend_ttl(&DataKey::GuardianSetIndex, TTL_THRESHOLD, TTL_EXTEND_TO);
    env.storage().persistent().extend_ttl(
        &DataKey::LastExecutedSequence,
        TTL_THRESHOLD,
        TTL_EXTEND_TO,
    );
}

/// Extend TTL on the contract instance storage.
pub fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
}
