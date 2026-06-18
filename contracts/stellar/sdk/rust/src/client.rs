use soroban_sdk::{vec, Address, Bytes, Env, IntoVal, Symbol};

use crate::error::ParseError;
use crate::payload::{parse_payload, Update};

/// Typed client for cross-contract calls to a deployed `pyth-lazer-stellar`
/// verifier contract.
pub struct PythLazerClient<'a> {
    env: &'a Env,
    address: Address,
}

impl<'a> PythLazerClient<'a> {
    /// Create a client bound to the verifier contract at `address`.
    pub fn new(env: &'a Env, address: &Address) -> Self {
        Self {
            env,
            address: address.clone(),
        }
    }

    /// Verify an LE-ECDSA signed Pyth Lazer update via the verifier contract and
    /// parse the verified payload into a typed [`Update`]. Traps if the update
    /// fails verification; returns [`ParseError`] if the verified payload bytes
    /// are malformed.
    pub fn verify_update(&self, data: &Bytes) -> Result<Update, ParseError> {
        let verified: Bytes = self.env.invoke_contract(
            &self.address,
            &Symbol::new(self.env, "verify_update"),
            vec![self.env, data.into_val(self.env)],
        );
        parse_payload(&verified)
    }
}
