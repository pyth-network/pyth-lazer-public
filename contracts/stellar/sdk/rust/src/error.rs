use soroban_sdk::contracterror;

/// Errors returned by [`crate::parse_payload`] when decoding a verified Lazer
/// payload. Declared as a `#[contracterror]` so consumers can propagate it
/// directly from their own contract entrypoints.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ParseError {
    TruncatedData = 1,
    InvalidPayloadLength = 2,
    InvalidPayloadMagic = 3,
    InvalidChannel = 4,
    InvalidProperty = 5,
    InvalidMarketSession = 6,
}
