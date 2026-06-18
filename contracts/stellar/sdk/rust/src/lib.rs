#![no_std]

//! Rust SDK for consuming [Pyth Lazer](https://docs.pyth.network/lazer) price
//! updates in Soroban (Stellar) contracts.
//!
//! Use [`PythLazerClient`] to cross-contract call a deployed
//! `pyth-lazer-stellar` verifier and [`parse_payload`] to decode the verified
//! payload bytes into typed values.

extern crate alloc;

mod client;
mod error;
mod payload;

pub use client::PythLazerClient;
pub use error::ParseError;
pub use payload::{parse_payload, Channel, Feed, MarketSession, Update};
