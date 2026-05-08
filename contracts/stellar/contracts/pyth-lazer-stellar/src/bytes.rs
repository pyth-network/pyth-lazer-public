use soroban_sdk::{Bytes, BytesN};

use crate::error::ContractError;

pub fn or_truncated<T>(value: Option<T>) -> Result<T, ContractError> {
    value.ok_or(ContractError::TruncatedData)
}

pub fn get_byte(data: &Bytes, index: u32) -> Result<u8, ContractError> {
    or_truncated(data.get(index))
}

pub fn get_byte_n<const N: usize>(data: &BytesN<N>, index: u32) -> Result<u8, ContractError> {
    or_truncated(data.get(index))
}
