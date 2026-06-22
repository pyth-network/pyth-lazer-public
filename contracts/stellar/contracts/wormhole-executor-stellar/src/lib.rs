#![no_std]

pub mod bytes;
pub mod error;
pub mod governance;
pub mod guardian;
pub mod vaa;

#[cfg(test)]
mod test;

use soroban_sdk::{
    contract, contractimpl, xdr::FromXdr, Bytes, BytesN, Env, Symbol, TryFromVal, Val, Vec,
};

use crate::bytes::{get_byte, read_be_u16, read_be_u32};
use crate::error::ContractError;
use crate::governance::{parse_ptgm, GovernanceAction, MAX_SYMBOL_LEN};
use crate::vaa::{parse_vaa, verify_vaa};

/// Wormhole core governance module identifier ("Core" right-aligned in 32 bytes).
const CORE_MODULE: [u8; 32] = *b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00Core";

/// Guardian set upgrade action ID.
const GUARDIAN_SET_UPGRADE_ACTION: u8 = 2;

#[contract]
pub struct WormholeExecutor;

#[contractimpl]
impl WormholeExecutor {
    /// Constructor for the executor contract.
    ///
    /// Runs only during deployment. Sets up the guardian set, chain ID, and
    /// emitter configuration.
    ///
    /// Two emitters are configured separately:
    /// - `owner_emitter_*` authorizes governance actions executed against
    ///   target contracts via [`Self::execute_governance_action`].
    /// - `gs_upgrade_emitter_*` (the *guardian-set-upgrade* emitter)
    ///   authorizes guardian set upgrade VAAs handled by
    ///   [`Self::update_guardian_set`]. In production this is the Wormhole
    ///   core bridge governance emitter, which is distinct from the Pyth
    ///   governance emitter used to issue PTGM actions.
    #[allow(clippy::too_many_arguments)]
    pub fn __constructor(
        env: Env,
        chain_id: u32,
        owner_emitter_chain: u32,
        owner_emitter_address: BytesN<32>,
        gs_upgrade_emitter_chain: u32,
        gs_upgrade_emitter_address: BytesN<32>,
        initial_guardian_set: Vec<BytesN<20>>,
        guardian_set_index: u32,
    ) -> Result<(), ContractError> {
        guardian::initialize(
            &env,
            chain_id,
            owner_emitter_chain,
            owner_emitter_address,
            gs_upgrade_emitter_chain,
            gs_upgrade_emitter_address,
            initial_guardian_set,
            guardian_set_index,
        )
    }

    /// Process a guardian set upgrade VAA.
    ///
    /// This is a self-governance mechanism: the current guardian set signs a VAA
    /// that authorizes a new guardian set. The VAA payload follows the Wormhole
    /// core governance format:
    ///
    /// ```text
    /// [32 bytes] module ("Core" right-padded)
    /// [1 byte]   action (2 = guardian set upgrade)
    /// [2 bytes]  target chain (0 = all chains)
    /// [4 bytes]  new guardian set index (BE u32)
    /// [1 byte]   num guardians
    /// For each guardian:
    ///   [20 bytes] Ethereum address
    /// ```
    ///
    /// Guardian set upgrades are governance actions and require the *current*
    /// guardian set: the VAA's `guardian_set_index` must equal the stored
    /// index, otherwise it is rejected with `InvalidGuardianSetIndex`. The
    /// 24-hour grace window in `verify_vaa` — during which a retired set's VAAs
    /// are still accepted — applies to non-governance message VAAs only and
    /// does not extend to this function.
    pub fn update_guardian_set(env: Env, vaa_bytes: Bytes) -> Result<(), ContractError> {
        guardian::require_initialized(&env)?;
        guardian::extend_instance_ttl(&env);

        // Parse and verify the VAA with current guardian set.
        let vaa = parse_vaa(&env, &vaa_bytes)?;
        verify_vaa(&env, &vaa)?;

        // Governance actions require the *current* guardian set. The 24-hour
        // grace window in `verify_vaa` applies to non-governance message VAAs
        // only; allowing a retired-but-in-window set to authorize a guardian
        // set upgrade would let a quorum of the old keys install a malicious
        // set N+1 for up to 24 hours after any rotation.
        if vaa.guardian_set_index != guardian::get_guardian_set_index(&env)? {
            return Err(ContractError::InvalidGuardianSetIndex);
        }

        // Validate emitter chain matches the stored guardian-set-upgrade
        // emitter. Without this check, any guardian-signed VAA from any
        // emitter could upgrade the guardian set if its payload happened to
        // be a valid Core/action=2 message. This emitter is distinct from
        // the governance owner emitter — guardian set upgrades come from the
        // Wormhole core bridge governance emitter, while target-contract
        // governance actions come from the Pyth governance emitter.
        let upgrade_chain = guardian::get_gs_upgrade_emitter_chain(&env)?;
        if vaa.body.emitter_chain as u32 != upgrade_chain {
            return Err(ContractError::InvalidEmitterChain);
        }

        // Validate emitter address matches the stored guardian-set-upgrade emitter.
        let upgrade_address = guardian::get_gs_upgrade_emitter_address(&env)?;
        if vaa.body.emitter_address != upgrade_address {
            return Err(ContractError::InvalidEmitterAddress);
        }

        let payload = &vaa.body.payload;
        let payload_len = payload.len() as usize;

        // Minimum payload size: 32 (module) + 1 (action) + 2 (chain) + 4 (index) + 1 (num) = 40
        if payload_len < 40 {
            return Err(ContractError::TruncatedData);
        }

        // Verify module is "Core".
        let mut module_bytes = [0u8; 32];
        for (i, slot) in module_bytes.iter_mut().enumerate() {
            *slot = get_byte(payload, i as u32)?;
        }
        if module_bytes != CORE_MODULE {
            return Err(ContractError::InvalidGovernanceModule);
        }

        // Verify action is guardian set upgrade (2).
        let action = get_byte(payload, 32)?;
        if action != GUARDIAN_SET_UPGRADE_ACTION {
            return Err(ContractError::InvalidGovernanceAction);
        }

        // Target chain (2 bytes BE u16) - 0 means all chains.
        let target_chain = read_be_u16(payload, 33)?;
        let our_chain = guardian::get_chain_id(&env)?;
        if target_chain != 0 && target_chain as u32 != our_chain {
            return Err(ContractError::InvalidTargetChain);
        }

        // New guardian set index (4 bytes BE u32).
        let new_index = read_be_u32(payload, 35)?;

        // Must increment by exactly 1.
        let current_index = guardian::get_guardian_set_index(&env)?;
        if new_index != current_index + 1 {
            return Err(ContractError::InvalidGuardianSetUpgrade);
        }

        // Number of guardians.
        let num_guardians = get_byte(payload, 39)? as usize;
        if num_guardians == 0 {
            return Err(ContractError::EmptyGuardianSet);
        }

        // Verify payload has enough data for all guardian addresses.
        let required_len = 40 + num_guardians * 20;
        if payload_len < required_len {
            return Err(ContractError::TruncatedData);
        }

        // Parse guardian Ethereum addresses.
        let mut new_guardian_set: Vec<BytesN<20>> = Vec::new(&env);
        for i in 0..num_guardians {
            let offset = 40 + i * 20;
            let mut addr = [0u8; 20];
            for (j, slot) in addr.iter_mut().enumerate() {
                *slot = get_byte(payload, (offset + j) as u32)?;
            }
            new_guardian_set.push_back(BytesN::from_array(&env, &addr));
        }

        // Store the new guardian set.
        guardian::store_guardian_set(&env, new_guardian_set, new_index);

        Ok(())
    }

    /// Execute a governance action from a Wormhole VAA containing a PTGM payload.
    ///
    /// This verifies the VAA, validates the emitter matches the stored owner,
    /// enforces replay protection via strictly increasing sequence numbers,
    /// parses the PTGM governance instruction, and dispatches a cross-contract
    /// call to the target contract.
    ///
    /// Like Pyth's other governance receivers, this relies on the generic
    /// `verify_vaa` path and so accepts a retired guardian set within its
    /// 24-hour grace window — it does *not* additionally require the current
    /// set. (The self-referential `update_guardian_set` upgrade path does
    /// require the current set; see its docs.)
    pub fn execute_governance_action(env: Env, vaa_bytes: Bytes) -> Result<(), ContractError> {
        guardian::require_initialized(&env)?;
        guardian::extend_instance_ttl(&env);

        // Parse and verify the VAA with current guardian set.
        let vaa = parse_vaa(&env, &vaa_bytes)?;
        verify_vaa(&env, &vaa)?;

        // Validate emitter chain matches stored owner.
        let owner_chain = guardian::get_owner_emitter_chain(&env)?;
        if vaa.body.emitter_chain as u32 != owner_chain {
            return Err(ContractError::InvalidEmitterChain);
        }

        // Validate emitter address matches stored owner.
        let owner_address = guardian::get_owner_emitter_address(&env)?;
        if vaa.body.emitter_address != owner_address {
            return Err(ContractError::InvalidEmitterAddress);
        }

        // Replay protection: sequence must be strictly increasing.
        let last_sequence = guardian::get_last_executed_sequence(&env);
        if vaa.body.sequence <= last_sequence {
            return Err(ContractError::StaleSequence);
        }

        // Update last executed sequence.
        guardian::set_last_executed_sequence(&env, vaa.body.sequence);

        // Parse the PTGM governance instruction. The payload carries a fully
        // generic call descriptor (target contract, function name, args) so
        // the executor can dispatch to any governance-protected function
        // without code changes per action.
        let our_chain = guardian::get_chain_id(&env)?;
        let action = parse_ptgm(
            &vaa.body.payload,
            our_chain,
            &env.current_contract_address(),
        )?;

        match action {
            GovernanceAction::Call(payload) => {
                let func = decode_function_symbol(&env, &payload.function_name)?;
                let args = Vec::<Val>::from_xdr(&env, &payload.args_xdr)
                    .map_err(|_| ContractError::InvalidArgsEncoding)?;
                env.invoke_contract::<()>(&payload.target_contract, &func, args);
            }
            GovernanceAction::UpgradeExecutor(payload) => {
                let wasm_hash = BytesN::from_array(&env, &payload.wasm_digest);
                env.deployer().update_current_contract_wasm(wasm_hash);
            }
        }

        Ok(())
    }
}

/// Decode raw UTF-8 bytes from a PTGM call payload into a Soroban `Symbol`.
///
/// Length is re-checked here even though `parse_ptgm` already gates it, so
/// any future call site that builds a `CallPayload` differently still gets a
/// safe conversion.
fn decode_function_symbol(env: &Env, name: &Bytes) -> Result<Symbol, ContractError> {
    let len = name.len() as usize;
    if len == 0 || len > MAX_SYMBOL_LEN {
        return Err(ContractError::InvalidFunctionName);
    }
    let mut buf = [0u8; MAX_SYMBOL_LEN];
    for (i, slot) in buf.iter_mut().take(len).enumerate() {
        *slot = get_byte(name, i as u32)?;
    }
    let s = core::str::from_utf8(&buf[..len]).map_err(|_| ContractError::InvalidFunctionName)?;
    Symbol::try_from_val(env, &s).map_err(|_| ContractError::InvalidFunctionName)
}
