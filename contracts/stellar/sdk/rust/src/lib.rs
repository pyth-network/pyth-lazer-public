#![no_std]

//! Rust SDK for consuming [Pyth Lazer](https://docs.pyth.network/lazer) price
//! updates in Soroban (Stellar) contracts.
//!
//! Use [`PythLazerClient`] to cross-contract call a deployed
//! `pyth-lazer-stellar` verifier and [`parse_payload`] to decode the verified
//! payload bytes into typed values.
//!
//! # Security
//!
//! [`parse_payload`] performs **no** signature verification — it decodes
//! whatever bytes it is handed. Only [`PythLazerClient::verify_update`]
//! establishes the trust boundary: it verifies the update via the on-chain
//! verifier contract before parsing and returns a [`VerifiedPayload`]. Passing
//! unverified, attacker-supplied bytes to [`parse_payload`] produces a
//! fully-formed but **unsigned** [`Update`] whose values must not be trusted.

extern crate alloc;

mod client;
mod error;
mod payload;

pub use client::{PythLazerClient, VerifiedPayload};
pub use error::ParseError;
pub use payload::{parse_payload, Channel, Feed, MarketSession, Update};
