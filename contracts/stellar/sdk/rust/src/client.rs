use core::ops::Deref;

use soroban_sdk::{vec, Address, Bytes, Env, IntoVal, Symbol};

use crate::error::ParseError;
use crate::payload::{parse_payload, Update};

/// An [`Update`] that has been verified through the on-chain verifier contract.
///
/// This newtype is the type-level guarantee that the wrapped [`Update`] came
/// from a successful [`PythLazerClient::verify_update`] rather than from a raw
/// [`parse_payload`] call on arbitrary bytes. Read fields directly via [`Deref`]
/// or take ownership of the inner value with [`VerifiedPayload::into_inner`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedPayload(Update);

impl VerifiedPayload {
    /// Consume the wrapper and return the verified [`Update`].
    pub fn into_inner(self) -> Update {
        self.0
    }
}

impl Deref for VerifiedPayload {
    type Target = Update;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

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
    /// parse the verified payload into a typed [`VerifiedPayload`]. Traps if the
    /// update fails verification; returns [`ParseError`] if the verified payload
    /// bytes are malformed.
    pub fn verify_update(&self, data: &Bytes) -> Result<VerifiedPayload, ParseError> {
        let verified: Bytes = self.env.invoke_contract(
            &self.address,
            &Symbol::new(self.env, "verify_update"),
            vec![self.env, data.into_val(self.env)],
        );
        parse_payload(&verified).map(VerifiedPayload)
    }
}
