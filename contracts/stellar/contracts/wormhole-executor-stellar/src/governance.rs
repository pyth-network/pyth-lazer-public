use soroban_sdk::{Address, Bytes};

use crate::bytes::{get_byte, read_be_u16, read_be_u64};
use crate::error::ContractError;

/// PTGM magic: "PTGM" = 0x5054474d
const PTGM_MAGIC: [u8; 4] = [0x50, 0x54, 0x47, 0x4d];

/// Module 3 = Lazer.
const LAZER_MODULE: u8 = 3;

/// Governance action: upgrade the target contract.
pub const ACTION_UPGRADE: u8 = 0;

/// Governance action: update a trusted signer on the target contract.
pub const ACTION_UPDATE_TRUSTED_SIGNER: u8 = 1;

/// Governance action: upgrade the executor contract itself.
pub const ACTION_UPGRADE_EXECUTOR: u8 = 2;

/// PTGM fixed header size: 4 (magic) + 1 (module) + 1 (action) + 2 (chain_id) = 8 bytes.
const HEADER_SIZE: usize = 8;

/// Parsed PTGM header.
#[derive(Clone, Debug, PartialEq)]
pub struct PtgmHeader {
    pub action: u8,
    pub target_chain_id: u16,
}

/// Parsed update_trusted_signer payload.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateTrustedSignerPayload {
    pub target_contract: Address,
    pub pubkey: [u8; 33],
    pub expires_at: u64,
}

/// Parsed upgrade payload.
#[derive(Clone, Debug, PartialEq)]
pub struct UpgradePayload {
    pub target_contract: Address,
    pub wasm_digest: [u8; 32],
}

/// Parsed upgrade-executor payload (self-upgrade).
#[derive(Clone, Debug, PartialEq)]
pub struct UpgradeExecutorPayload {
    pub target_contract: Address,
    pub wasm_digest: [u8; 32],
}

/// Parsed governance action from a PTGM.
#[derive(Clone, Debug, PartialEq)]
pub enum GovernanceAction {
    Upgrade(UpgradePayload),
    UpdateTrustedSigner(UpdateTrustedSignerPayload),
    UpgradeExecutor(UpgradeExecutorPayload),
}

/// Parse a PTGM from a VAA payload.
///
/// Format:
/// - [4 bytes] magic "PTGM"
/// - [1 byte]  module (3 = Lazer)
/// - [1 byte]  action (0 = upgrade, 1 = update_trusted_signer)
/// - [2 bytes] target_chain_id (BE u16)
/// - action-specific payload
pub fn parse_ptgm(
    payload: &Bytes,
    our_chain_id: u32,
    expected_executor_contract: &Address,
) -> Result<GovernanceAction, ContractError> {
    let len = payload.len() as usize;

    if len < HEADER_SIZE {
        return Err(ContractError::TruncatedData);
    }

    // Verify magic.
    for (i, expected) in PTGM_MAGIC.iter().enumerate() {
        if get_byte(payload, i as u32)? != *expected {
            return Err(ContractError::InvalidPtgmMagic);
        }
    }

    // Verify module.
    let module = get_byte(payload, 4)?;
    if module != LAZER_MODULE {
        return Err(ContractError::InvalidPtgmModule);
    }

    // Action.
    let action = get_byte(payload, 5)?;

    // Target chain ID (BE u16).
    let target_chain_id = read_be_u16(payload, 6)?;

    // Validate target chain: must match our chain (no wildcard 0 for PTGM).
    if target_chain_id as u32 != our_chain_id {
        return Err(ContractError::InvalidTargetChain);
    }

    // Routing metadata encoded in the PTGM payload:
    // - [1 byte]  executor contract address strkey length
    // - [N bytes] executor contract address strkey
    // - [1 byte]  target contract address strkey length
    // - [M bytes] target contract address strkey
    // (Note: stellar addresses are variable length, hence the length fields)
    let mut offset = HEADER_SIZE;
    if len < offset + 2 {
        return Err(ContractError::TruncatedData);
    }

    let executor_len = get_byte(payload, offset as u32)? as usize;
    offset += 1;
    if executor_len == 0 || len < offset + executor_len {
        return Err(ContractError::TruncatedData);
    }
    let executor_contract =
        Address::from_string_bytes(&payload.slice(offset as u32..(offset + executor_len) as u32));
    offset += executor_len;
    if &executor_contract != expected_executor_contract {
        return Err(ContractError::InvalidExecutorAddress);
    }

    if len < offset + 1 {
        return Err(ContractError::TruncatedData);
    }
    let target_len = get_byte(payload, offset as u32)? as usize;
    offset += 1;
    if target_len == 0 || len < offset + target_len {
        return Err(ContractError::TruncatedData);
    }
    let target_contract =
        Address::from_string_bytes(&payload.slice(offset as u32..(offset + target_len) as u32));
    offset += target_len;

    match action {
        ACTION_UPDATE_TRUSTED_SIGNER => {
            // 33 (pubkey) + 8 (expires_at) = 41 bytes after header.
            if len < offset + 41 {
                return Err(ContractError::TruncatedData);
            }

            let mut pubkey = [0u8; 33];
            for (i, slot) in pubkey.iter_mut().enumerate() {
                *slot = get_byte(payload, (offset + i) as u32)?;
            }

            let exp_offset = offset + 33;
            let expires_at = read_be_u64(payload, exp_offset as u32)?;

            Ok(GovernanceAction::UpdateTrustedSigner(
                UpdateTrustedSignerPayload {
                    target_contract,
                    pubkey,
                    expires_at,
                },
            ))
        }
        ACTION_UPGRADE => {
            // 32 (wasm_digest) bytes after header.
            if len < offset + 32 {
                return Err(ContractError::TruncatedData);
            }

            let mut wasm_digest = [0u8; 32];
            for (i, slot) in wasm_digest.iter_mut().enumerate() {
                *slot = get_byte(payload, (offset + i) as u32)?;
            }

            Ok(GovernanceAction::Upgrade(UpgradePayload {
                target_contract,
                wasm_digest,
            }))
        }
        ACTION_UPGRADE_EXECUTOR => {
            // Self-upgrade: target contract must be the executor itself.
            if &target_contract != expected_executor_contract {
                return Err(ContractError::InvalidTargetContract);
            }

            // 32 (wasm_digest) bytes after header.
            if len < offset + 32 {
                return Err(ContractError::TruncatedData);
            }

            let mut wasm_digest = [0u8; 32];
            for (i, slot) in wasm_digest.iter_mut().enumerate() {
                *slot = get_byte(payload, (offset + i) as u32)?;
            }

            Ok(GovernanceAction::UpgradeExecutor(UpgradeExecutorPayload {
                target_contract,
                wasm_digest,
            }))
        }
        _ => Err(ContractError::InvalidGovernanceAction),
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use soroban_sdk::{testutils::Address as _, Address, Env};

    const TEST_CHAIN_ID: u32 = 30;

    fn address_to_payload_bytes(address: &Address) -> alloc::vec::Vec<u8> {
        let strkey = address.to_string();
        let mut out = alloc::vec![0u8; strkey.len() as usize];
        strkey.copy_into_slice(&mut out);
        out
    }

    fn build_ptgm_header(
        module: u8,
        action: u8,
        chain_id: u16,
        executor_contract: &Address,
        target_contract: &Address,
    ) -> alloc::vec::Vec<u8> {
        let mut data = alloc::vec::Vec::new();
        data.extend_from_slice(&PTGM_MAGIC);
        data.push(module);
        data.push(action);
        data.extend_from_slice(&chain_id.to_be_bytes());
        let executor = address_to_payload_bytes(executor_contract);
        data.push(executor.len() as u8);
        data.extend_from_slice(&executor);
        let target = address_to_payload_bytes(target_contract);
        data.push(target.len() as u8);
        data.extend_from_slice(&target);
        data
    }

    fn build_update_trusted_signer_ptgm(
        chain_id: u16,
        executor_contract: &Address,
        target_contract: &Address,
        pubkey: &[u8; 33],
        expires_at: u64,
    ) -> alloc::vec::Vec<u8> {
        let mut data = build_ptgm_header(
            LAZER_MODULE,
            ACTION_UPDATE_TRUSTED_SIGNER,
            chain_id,
            executor_contract,
            target_contract,
        );
        data.extend_from_slice(pubkey);
        data.extend_from_slice(&expires_at.to_be_bytes());
        data
    }

    fn build_upgrade_ptgm(
        chain_id: u16,
        executor_contract: &Address,
        target_contract: &Address,
        wasm_digest: &[u8; 32],
    ) -> alloc::vec::Vec<u8> {
        let mut data = build_ptgm_header(
            LAZER_MODULE,
            ACTION_UPGRADE,
            chain_id,
            executor_contract,
            target_contract,
        );
        data.extend_from_slice(wasm_digest);
        data
    }

    #[test]
    fn test_parse_update_trusted_signer() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let pubkey = [0xAA; 33];
        let expires_at = 1700000000u64;

        let raw = build_update_trusted_signer_ptgm(
            TEST_CHAIN_ID as u16,
            &executor,
            &target,
            &pubkey,
            expires_at,
        );
        let payload = Bytes::from_slice(&env, &raw);

        let result = parse_ptgm(&payload, TEST_CHAIN_ID, &executor).unwrap();
        match result {
            GovernanceAction::UpdateTrustedSigner(p) => {
                assert_eq!(p.target_contract, target);
                assert_eq!(p.pubkey, pubkey);
                assert_eq!(p.expires_at, expires_at);
            }
            _ => panic!("expected UpdateTrustedSigner"),
        }
    }

    #[test]
    fn test_parse_upgrade() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let wasm_digest = [0xBB; 32];

        let raw = build_upgrade_ptgm(TEST_CHAIN_ID as u16, &executor, &target, &wasm_digest);
        let payload = Bytes::from_slice(&env, &raw);

        let result = parse_ptgm(&payload, TEST_CHAIN_ID, &executor).unwrap();
        match result {
            GovernanceAction::Upgrade(p) => {
                assert_eq!(p.target_contract, target);
                assert_eq!(p.wasm_digest, wasm_digest);
            }
            _ => panic!("expected Upgrade"),
        }
    }

    #[test]
    fn test_invalid_magic() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let mut raw =
            build_update_trusted_signer_ptgm(TEST_CHAIN_ID as u16, &executor, &target, &[0; 33], 0);
        raw[0] = 0xFF; // corrupt magic
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::InvalidPtgmMagic)
        );
    }

    #[test]
    fn test_invalid_module() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let mut raw = build_ptgm_header(
            99,
            ACTION_UPDATE_TRUSTED_SIGNER,
            TEST_CHAIN_ID as u16,
            &executor,
            &target,
        );
        // Add enough payload for update_trusted_signer.
        raw.extend_from_slice(&[0u8; 41]);
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::InvalidPtgmModule)
        );
    }

    #[test]
    fn test_invalid_target_chain() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let raw = build_update_trusted_signer_ptgm(99, &executor, &target, &[0; 33], 0); // chain 99 != 30
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::InvalidTargetChain)
        );
    }

    #[test]
    fn test_invalid_action() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let mut raw = build_ptgm_header(LAZER_MODULE, 99, TEST_CHAIN_ID as u16, &executor, &target);
        raw.extend_from_slice(&[0u8; 41]);
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::InvalidGovernanceAction)
        );
    }

    #[test]
    fn test_truncated_header() {
        let env = Env::default();
        let payload = Bytes::from_slice(&env, &[0x50, 0x54, 0x47]); // only 3 bytes
        let expected_executor = Address::generate(&env);
        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &expected_executor).err(),
            Some(ContractError::TruncatedData)
        );
    }

    #[test]
    fn test_truncated_update_trusted_signer_payload() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let mut raw = build_ptgm_header(
            LAZER_MODULE,
            ACTION_UPDATE_TRUSTED_SIGNER,
            TEST_CHAIN_ID as u16,
            &executor,
            &target,
        );
        raw.extend_from_slice(&[0u8; 20]); // not enough for 33 + 8 = 41 bytes
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::TruncatedData)
        );
    }

    #[test]
    fn test_truncated_upgrade_payload() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let mut raw = build_ptgm_header(
            LAZER_MODULE,
            ACTION_UPGRADE,
            TEST_CHAIN_ID as u16,
            &executor,
            &target,
        );
        raw.extend_from_slice(&[0u8; 10]); // not enough for 32 bytes
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::TruncatedData)
        );
    }

    #[test]
    fn test_parse_max_expires_at() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let raw = build_update_trusted_signer_ptgm(
            TEST_CHAIN_ID as u16,
            &executor,
            &target,
            &[0xFF; 33],
            u64::MAX,
        );
        let payload = Bytes::from_slice(&env, &raw);

        let result = parse_ptgm(&payload, TEST_CHAIN_ID, &executor).unwrap();
        match result {
            GovernanceAction::UpdateTrustedSigner(p) => {
                assert_eq!(p.target_contract, target);
                assert_eq!(p.expires_at, u64::MAX);
                assert_eq!(p.pubkey, [0xFF; 33]);
            }
            _ => panic!("expected UpdateTrustedSigner"),
        }
    }

    #[test]
    fn test_invalid_executor_address() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let wrong_executor = Address::generate(&env);
        let target = Address::generate(&env);
        let raw = build_update_trusted_signer_ptgm(
            TEST_CHAIN_ID as u16,
            &executor,
            &target,
            &[0xAA; 33],
            1000,
        );
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &wrong_executor).err(),
            Some(ContractError::InvalidExecutorAddress)
        );
    }

    fn build_upgrade_executor_ptgm(
        chain_id: u16,
        executor_contract: &Address,
        target_contract: &Address,
        wasm_digest: &[u8; 32],
    ) -> alloc::vec::Vec<u8> {
        let mut data = build_ptgm_header(
            LAZER_MODULE,
            ACTION_UPGRADE_EXECUTOR,
            chain_id,
            executor_contract,
            target_contract,
        );
        data.extend_from_slice(wasm_digest);
        data
    }

    #[test]
    fn test_parse_upgrade_executor() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let wasm_digest = [0xCC; 32];

        // Target contract == executor (self-upgrade).
        let raw =
            build_upgrade_executor_ptgm(TEST_CHAIN_ID as u16, &executor, &executor, &wasm_digest);
        let payload = Bytes::from_slice(&env, &raw);

        let result = parse_ptgm(&payload, TEST_CHAIN_ID, &executor).unwrap();
        match result {
            GovernanceAction::UpgradeExecutor(p) => {
                assert_eq!(p.target_contract, executor);
                assert_eq!(p.wasm_digest, wasm_digest);
            }
            _ => panic!("expected UpgradeExecutor"),
        }
    }

    #[test]
    fn test_upgrade_executor_wrong_target() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let other_contract = Address::generate(&env);

        // Target contract != executor — must be rejected.
        let raw = build_upgrade_executor_ptgm(
            TEST_CHAIN_ID as u16,
            &executor,
            &other_contract,
            &[0xDD; 32],
        );
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::InvalidTargetContract)
        );
    }

    #[test]
    fn test_truncated_upgrade_executor_payload() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let mut raw = build_ptgm_header(
            LAZER_MODULE,
            ACTION_UPGRADE_EXECUTOR,
            TEST_CHAIN_ID as u16,
            &executor,
            &executor,
        );
        raw.extend_from_slice(&[0u8; 10]); // not enough for 32 bytes
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::TruncatedData)
        );
    }
}
