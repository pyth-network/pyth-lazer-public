use soroban_sdk::{Address, Bytes};

use crate::bytes::{get_byte, read_be_u16};
use crate::error::ContractError;

/// PTGM magic: "PTGM" = 0x5054474d
const PTGM_MAGIC: [u8; 4] = [0x50, 0x54, 0x47, 0x4d];

/// PTGM module id for the Stellar wormhole executor.
///
/// The Stellar executor is its own governance module rather than reusing the
/// Lazer module (3). Module 3 carries the canonical `xc_admin_common`
/// `LazerAction` registry, whose fixed action codes (e.g. `UpgradeSuiLazerContract`,
/// `UpdateTrustedSigner`) collide with this executor's `(action, payload)`
/// layout. Giving the executor a dedicated module — mirroring the canonical
/// precedent where generic executors are separate modules (Solana
/// remote-executor `0`, EVM executor `2`) — keeps module 3 unambiguously
/// "Lazer fixed actions" and lets this executor own action codes `0`/`1`.
///
/// Module id `4` is the next free slot after the canonical modules (Executor
/// `0`, Target `1`, EvmExecutor `2`, Lazer `3`). The matching registration in
/// the EVM `Executor.sol` `GovernanceModule` enum and `xc_admin_common` lives
/// in `pyth-network/pyth-crosschain` and is tracked separately.
const STELLAR_EXECUTOR_MODULE: u8 = 4;

/// Governance action: upgrade the executor contract itself.
///
/// Kept as a separate action because it uses the `update_current_contract_wasm`
/// host function rather than a cross-contract `invoke_contract` call.
pub const ACTION_UPGRADE_EXECUTOR: u8 = 0;

/// Governance action: generic call to any function on a target contract.
///
/// The executor invokes `function_name` on `target_contract` with the XDR-encoded
/// `args` (an `ScVec` whose elements are arbitrary `ScVal`s). This replaces the
/// previous per-function action IDs so that the executor can dispatch governance
/// calls to any function on any contract without further code changes.
pub const ACTION_CALL: u8 = 1;

/// PTGM fixed header size: 4 (magic) + 1 (module) + 1 (action) + 2 (chain_id) = 8 bytes.
const HEADER_SIZE: usize = 8;

/// Soroban `Symbol` maximum length, in bytes.
pub const MAX_SYMBOL_LEN: usize = 32;

/// Parsed PTGM header.
#[derive(Clone, Debug, PartialEq)]
pub struct PtgmHeader {
    pub action: u8,
    pub target_chain_id: u16,
}

/// Parsed generic-call payload.
///
/// `function_name` is the raw UTF-8 bytes of a Soroban `Symbol` (length is
/// pre-validated to be in `1..=MAX_SYMBOL_LEN`). `args_xdr` is the XDR encoding
/// of an `ScVec` whose elements are the arguments to pass to the target
/// function. The dispatcher in `lib.rs` decodes these into a `Symbol` and a
/// `Vec<Val>` respectively before calling `env.invoke_contract`.
#[derive(Clone, Debug, PartialEq)]
pub struct CallPayload {
    pub target_contract: Address,
    pub function_name: Bytes,
    pub args_xdr: Bytes,
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
    Call(CallPayload),
    UpgradeExecutor(UpgradeExecutorPayload),
}

/// Parse a PTGM from a VAA payload.
///
/// Common header:
/// - [4 bytes] magic "PTGM"
/// - [1 byte]  module (4 = Stellar executor)
/// - [1 byte]  action (0 = upgrade executor, 1 = generic call)
/// - [2 bytes] target_chain_id (BE u16)
/// - [1 byte]  executor strkey length
/// - [N bytes] executor strkey (must equal `expected_executor_contract`)
/// - [1 byte]  target strkey length
/// - [M bytes] target strkey
///
/// `ACTION_CALL` payload:
/// - [1 byte]  function name length (1..=32)
/// - [F bytes] function name (UTF-8, valid Soroban `Symbol` characters)
/// - [remainder] XDR-encoded `ScVec` of call arguments
///
/// `ACTION_UPGRADE_EXECUTOR` payload (target_contract must equal the executor):
/// - [32 bytes] wasm digest
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
    if module != STELLAR_EXECUTOR_MODULE {
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
    // `from_string_bytes` traps on malformed strkey rather than returning a
    // typed error. soroban-sdk exposes no fallible variant (the strkey host
    // functions are infallible from guest code, so any error unwinds the VM),
    // and we deliberately do not hand-roll a strkey validator as a workaround.
    // Safe to trap here: these bytes are only reachable inside a
    // guardian-quorum- and owner/emitter-verified VAA, so an operator typo in
    // an already-signed payload fails closed rather than executing. Same
    // applies to the target address below.
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
        ACTION_CALL => {
            if len < offset + 1 {
                return Err(ContractError::TruncatedData);
            }
            let name_len = get_byte(payload, offset as u32)? as usize;
            offset += 1;
            if name_len == 0 || name_len > MAX_SYMBOL_LEN {
                return Err(ContractError::InvalidFunctionName);
            }
            if len < offset + name_len {
                return Err(ContractError::TruncatedData);
            }
            let function_name = payload.slice(offset as u32..(offset + name_len) as u32);
            offset += name_len;

            // Args occupy the rest of the payload. May be zero bytes only if
            // the caller intends to encode an empty ScVec — typically still
            // ≥ 8 bytes of XDR. We don't enforce a minimum here; XDR decode
            // failure in the dispatcher surfaces as InvalidArgsEncoding.
            let args_xdr = payload.slice(offset as u32..len as u32);

            Ok(GovernanceAction::Call(CallPayload {
                target_contract,
                function_name,
                args_xdr,
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
    use soroban_sdk::{
        testutils::Address as _, xdr::ToXdr, Address, BytesN, Env, IntoVal, Val, Vec,
    };

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

    fn build_call_ptgm(
        chain_id: u16,
        executor_contract: &Address,
        target_contract: &Address,
        function_name: &[u8],
        args_xdr: &[u8],
    ) -> alloc::vec::Vec<u8> {
        let mut data = build_ptgm_header(
            STELLAR_EXECUTOR_MODULE,
            ACTION_CALL,
            chain_id,
            executor_contract,
            target_contract,
        );
        data.push(function_name.len() as u8);
        data.extend_from_slice(function_name);
        data.extend_from_slice(args_xdr);
        data
    }

    /// Encode a `Vec<Val>` (the form `invoke_contract` expects) to its XDR
    /// `ScVec` bytes.
    fn encode_args(env: &Env, args: Vec<Val>) -> alloc::vec::Vec<u8> {
        args.to_xdr(env).to_alloc_vec()
    }

    #[test]
    fn test_parse_call_update_trusted_signer() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let pubkey = BytesN::from_array(&env, &[0xAA; 33]);
        let expires_at = 1_700_000_000u64;
        let args: Vec<Val> = (pubkey.clone(), expires_at).into_val(&env);
        let args_xdr = encode_args(&env, args);

        let raw = build_call_ptgm(
            TEST_CHAIN_ID as u16,
            &executor,
            &target,
            b"update_trusted_signer",
            &args_xdr,
        );
        let payload = Bytes::from_slice(&env, &raw);

        let result = parse_ptgm(&payload, TEST_CHAIN_ID, &executor).unwrap();
        match result {
            GovernanceAction::Call(p) => {
                assert_eq!(p.target_contract, target);
                assert_eq!(
                    p.function_name,
                    Bytes::from_slice(&env, b"update_trusted_signer")
                );
                assert_eq!(p.args_xdr, Bytes::from_slice(&env, &args_xdr));
            }
            _ => panic!("expected Call"),
        }
    }

    #[test]
    fn test_parse_call_upgrade() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let wasm_hash = BytesN::from_array(&env, &[0xBB; 32]);
        let args: Vec<Val> = (wasm_hash,).into_val(&env);
        let args_xdr = encode_args(&env, args);

        let raw = build_call_ptgm(
            TEST_CHAIN_ID as u16,
            &executor,
            &target,
            b"upgrade",
            &args_xdr,
        );
        let payload = Bytes::from_slice(&env, &raw);

        let result = parse_ptgm(&payload, TEST_CHAIN_ID, &executor).unwrap();
        match result {
            GovernanceAction::Call(p) => {
                assert_eq!(p.target_contract, target);
                assert_eq!(p.function_name, Bytes::from_slice(&env, b"upgrade"));
                assert_eq!(p.args_xdr.len() as usize, args_xdr.len());
            }
            _ => panic!("expected Call"),
        }
    }

    #[test]
    fn test_parse_call_empty_args() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let empty_args: Vec<Val> = Vec::new(&env);
        let args_xdr = encode_args(&env, empty_args);

        let raw = build_call_ptgm(TEST_CHAIN_ID as u16, &executor, &target, b"ping", &args_xdr);
        let payload = Bytes::from_slice(&env, &raw);

        let result = parse_ptgm(&payload, TEST_CHAIN_ID, &executor).unwrap();
        match result {
            GovernanceAction::Call(p) => {
                assert_eq!(p.function_name, Bytes::from_slice(&env, b"ping"));
            }
            _ => panic!("expected Call"),
        }
    }

    #[test]
    fn test_invalid_magic() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let mut raw = build_call_ptgm(TEST_CHAIN_ID as u16, &executor, &target, b"f", &[]);
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
        let mut raw = build_ptgm_header(99, ACTION_CALL, TEST_CHAIN_ID as u16, &executor, &target);
        // Minimal call payload (name + empty args).
        raw.push(1);
        raw.push(b'f');
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
        let raw = build_call_ptgm(99, &executor, &target, b"f", &[]);
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
        let mut raw = build_ptgm_header(
            STELLAR_EXECUTOR_MODULE,
            99,
            TEST_CHAIN_ID as u16,
            &executor,
            &target,
        );
        // Add a few extra bytes so the action parses.
        raw.extend_from_slice(&[0u8; 4]);
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
    fn test_truncated_call_function_name_len() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        // Header only — no function-name length byte.
        let raw = build_ptgm_header(
            STELLAR_EXECUTOR_MODULE,
            ACTION_CALL,
            TEST_CHAIN_ID as u16,
            &executor,
            &target,
        );
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::TruncatedData)
        );
    }

    #[test]
    fn test_truncated_call_function_name() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let mut raw = build_ptgm_header(
            STELLAR_EXECUTOR_MODULE,
            ACTION_CALL,
            TEST_CHAIN_ID as u16,
            &executor,
            &target,
        );
        raw.push(8); // claims 8-byte name…
        raw.extend_from_slice(b"fn"); // …but only 2 bytes follow
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::TruncatedData)
        );
    }

    #[test]
    fn test_call_zero_length_function_name() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        let raw = build_call_ptgm(TEST_CHAIN_ID as u16, &executor, &target, b"", &[]);
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::InvalidFunctionName)
        );
    }

    #[test]
    fn test_call_function_name_too_long() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let target = Address::generate(&env);
        // 33-byte name exceeds the Symbol limit.
        let long_name = [b'a'; MAX_SYMBOL_LEN + 1];
        let raw = build_call_ptgm(TEST_CHAIN_ID as u16, &executor, &target, &long_name, &[]);
        let payload = Bytes::from_slice(&env, &raw);

        assert_eq!(
            parse_ptgm(&payload, TEST_CHAIN_ID, &executor).err(),
            Some(ContractError::InvalidFunctionName)
        );
    }

    #[test]
    fn test_invalid_executor_address() {
        let env = Env::default();
        let executor = Address::generate(&env);
        let wrong_executor = Address::generate(&env);
        let target = Address::generate(&env);
        let raw = build_call_ptgm(TEST_CHAIN_ID as u16, &executor, &target, b"some_fn", &[]);
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
            STELLAR_EXECUTOR_MODULE,
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
            STELLAR_EXECUTOR_MODULE,
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
