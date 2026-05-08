use soroban_sdk::Bytes;

use crate::error::ContractError;

pub fn or_truncated<T>(value: Option<T>) -> Result<T, ContractError> {
    value.ok_or(ContractError::TruncatedData)
}

pub fn get_byte(data: &Bytes, index: u32) -> Result<u8, ContractError> {
    or_truncated(data.get(index))
}

pub fn read_be_u16(data: &Bytes, offset: u32) -> Result<u16, ContractError> {
    Ok(((get_byte(data, offset)? as u16) << 8) | (get_byte(data, offset + 1)? as u16))
}

pub fn read_be_u32(data: &Bytes, offset: u32) -> Result<u32, ContractError> {
    Ok(((get_byte(data, offset)? as u32) << 24)
        | ((get_byte(data, offset + 1)? as u32) << 16)
        | ((get_byte(data, offset + 2)? as u32) << 8)
        | (get_byte(data, offset + 3)? as u32))
}

pub fn read_be_u64(data: &Bytes, offset: u32) -> Result<u64, ContractError> {
    Ok(((get_byte(data, offset)? as u64) << 56)
        | ((get_byte(data, offset + 1)? as u64) << 48)
        | ((get_byte(data, offset + 2)? as u64) << 40)
        | ((get_byte(data, offset + 3)? as u64) << 32)
        | ((get_byte(data, offset + 4)? as u64) << 24)
        | ((get_byte(data, offset + 5)? as u64) << 16)
        | ((get_byte(data, offset + 6)? as u64) << 8)
        | (get_byte(data, offset + 7)? as u64))
}
