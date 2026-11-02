//! Encryption of the session file on Windows with DPAPI (`CryptProtectData`). Only the same
//! Windows user can decrypt the data. Other users and copies of the file on other computers cannot.

use std::io;
use std::ptr;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows_sys::core::BOOL;

/// Extra data for the encryption. A different app that calls DPAPI without it cannot decrypt the
/// file.
const ENTROPY: &[u8] = b"Melodia session";

pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
    call(data, |input, entropy, output| {
        // SAFETY: all pointers are valid for the call. DPAPI shows no prompt with this flag.
        unsafe {
            CryptProtectData(
                input,
                ptr::null(),
                entropy,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                output,
            )
        }
    })
}

pub fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    call(data, |input, entropy, output| {
        // SAFETY: all pointers are valid for the call. DPAPI shows no prompt with this flag.
        unsafe {
            CryptUnprotectData(
                input,
                ptr::null_mut(),
                entropy,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                output,
            )
        }
    })
}

/// Runs a DPAPI function on `data`, and copies its output to a `Vec`.
fn call(
    data: &[u8],
    function: impl FnOnce(
        *const CRYPT_INTEGER_BLOB,
        *const CRYPT_INTEGER_BLOB,
        *mut CRYPT_INTEGER_BLOB,
    ) -> BOOL,
) -> io::Result<Vec<u8>> {
    let input = blob(data)?;
    let entropy = blob(ENTROPY)?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    if function(&raw const input, &raw const entropy, &raw mut output) == 0 {
        return Err(io::Error::last_os_error());
    }
    if output.pbData.is_null() {
        return Ok(Vec::new());
    }
    // SAFETY: on success, DPAPI gives `cbData` bytes at `pbData`, and the caller must release them
    // with `LocalFree`.
    unsafe {
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        Ok(bytes)
    }
}

/// DPAPI does not write to its input blobs.
fn blob(data: &[u8]) -> io::Result<CRYPT_INTEGER_BLOB> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).map_err(io::Error::other)?,
        pbData: data.as_ptr().cast_mut(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_data_is_encrypted_and_comes_back() {
        let secret = br#"{"refresh_token":"Bohemian Rhapsody"}"#;
        let protected = protect(secret).unwrap();
        assert!(!protected.windows(secret.len()).any(|w| w == secret));
        assert_eq!(unprotect(&protected).unwrap(), secret);
        assert!(unprotect(b"not a DPAPI blob").is_err());
    }
}
