use soroban_sdk::{contracttype, Address, BytesN, Env, Map, Vec};

use crate::error::ContractError;

/// TTL threshold: extend when TTL drops below this (approx 6 days at 5s/ledger).
pub const TTL_THRESHOLD: u32 = 100_000;
/// TTL extension target (approx 29 days at 5s/ledger).
pub const TTL_EXTEND_TO: u32 = 500_000;

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    /// The executor address authorized for governance operations.
    Executor,
    /// The full set of trusted signers, stored as a single enumerable map from
    /// compressed secp256k1 pubkey to expiry timestamp (unix seconds). A single
    /// map (rather than one storage entry per signer) lets the whole set be
    /// listed on-chain.
    TrustedSigners,
}

/// Store the executor address (one-time initialization).
pub fn set_executor(env: &Env, executor: &Address) {
    env.storage().instance().set(&DataKey::Executor, executor);
}

/// Read the executor address. Panics if not initialized.
pub fn get_executor(env: &Env) -> Result<Address, ContractError> {
    env.storage()
        .instance()
        .get(&DataKey::Executor)
        .ok_or(ContractError::NotInitialized)
}

/// Check whether the contract has been initialized.
pub fn has_executor(env: &Env) -> bool {
    env.storage().instance().has(&DataKey::Executor)
}

/// Read the trusted-signers map, defaulting to an empty map when unset.
fn get_trusted_signers(env: &Env) -> Map<BytesN<33>, u64> {
    env.storage()
        .persistent()
        .get(&DataKey::TrustedSigners)
        .unwrap_or_else(|| Map::new(env))
}

/// Persist the trusted-signers map and bump its TTL.
fn put_trusted_signers(env: &Env, signers: &Map<BytesN<33>, u64>) {
    env.storage()
        .persistent()
        .set(&DataKey::TrustedSigners, signers);
    env.storage()
        .persistent()
        .extend_ttl(&DataKey::TrustedSigners, TTL_THRESHOLD, TTL_EXTEND_TO);
}

/// Extend the TTL on the trusted-signers map (used on read paths).
fn extend_trusted_signers_ttl(env: &Env) {
    env.storage()
        .persistent()
        .extend_ttl(&DataKey::TrustedSigners, TTL_THRESHOLD, TTL_EXTEND_TO);
}

/// Store or update a trusted signer's expiry timestamp (unix seconds).
/// If `expires_at` is 0, removes the signer.
pub fn set_trusted_signer(env: &Env, pubkey: &BytesN<33>, expires_at: u64) {
    let mut signers = get_trusted_signers(env);
    if expires_at == 0 {
        signers.remove(pubkey.clone());
    } else {
        signers.set(pubkey.clone(), expires_at);
    }
    put_trusted_signers(env, &signers);
}

/// List every currently trusted signer as `(pubkey, expires_at)` pairs.
/// Extends the TTL on the map it reads.
pub fn list_trusted_signers(env: &Env) -> Vec<(BytesN<33>, u64)> {
    let signers = get_trusted_signers(env);
    if !signers.is_empty() {
        extend_trusted_signers_ttl(env);
    }
    let mut result = Vec::new(env);
    for (pubkey, expires_at) in signers.iter() {
        result.push_back((pubkey, expires_at));
    }
    result
}

/// Get a trusted signer's expiry timestamp. Returns None if not found.
pub fn get_trusted_signer_expiry(env: &Env, pubkey: &BytesN<33>) -> Option<u64> {
    let result = get_trusted_signers(env).get(pubkey.clone());
    if result.is_some() {
        extend_trusted_signers_ttl(env);
    }
    result
}

/// Extend TTL on instance storage (call on every user-facing invocation).
pub fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
}
