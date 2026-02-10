use crate::error::{NotHexError, UtilError};

pub mod logger;
pub mod shutdown;
pub mod task_tracker;
pub mod url_validation;

pub fn unhex(hex_data: &str, what_is_hex: NotHexError) -> Result<Vec<u8>, UtilError> {
    if let Some(stripped) = hex_data.strip_prefix("0x") {
        const_hex::decode(stripped).map_err(|_| UtilError::NotHex(what_is_hex))
    } else {
        const_hex::decode(hex_data).map_err(|_| UtilError::NotHex(what_is_hex))
    }
}
