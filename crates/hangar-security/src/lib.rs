use hangar_core::SecurityStatus;

pub fn base_security_status() -> SecurityStatus {
    let active_features = vec![
        "core".to_string(),
        #[cfg(feature = "mutation")]
        "mutation".to_string(),
        #[cfg(feature = "agent_automation")]
        "agent_automation".to_string(),
    ];
    SecurityStatus {
        outbound_network: if cfg!(feature = "agent_automation") {
            "opt-in AI Assist only (off by default): a single call to the user-configured provider \
             after the sensitive/secret send-gate; the API key stays in the OS keychain. No other \
             outbound."
                .to_string()
        } else {
            "not implemented in this build".to_string()
        },
        mutation_executor: if cfg!(feature = "mutation") {
            "feature-gated executor compiled; confirmation and plan checks remain mandatory"
                .to_string()
        } else {
            "not compiled in this strict core build".to_string()
        },
        agent_ipc: if cfg!(feature = "agent_automation") {
            "feature-gated authenticated local named pipe compiled; no external listener"
                .to_string()
        } else {
            "not compiled in this strict core build".to_string()
        },
        active_features,
        notes: vec![
            "Markdown preview blocks scripts and remote images.".to_string(),
            "Sensitive and Protected Zone files are excluded from preview and FTS.".to_string(),
            "The file-backed SQLite database is encrypted at rest with SQLCipher.".to_string(),
            "Remote URLs found in files are passive text only.".to_string(),
        ],
    }
}

#[cfg(windows)]
pub fn protect_local_bytes(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("Windows DPAPI failed to protect local cache bytes.".to_string());
    }
    let protected =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(protected)
}

#[cfg(not(windows))]
pub fn protect_local_bytes(_bytes: &[u8]) -> Result<Vec<u8>, String> {
    Err("Protected local cache bytes require Windows DPAPI in this phase.".to_string())
}

#[cfg(windows)]
pub fn unprotect_local_bytes(protected: &[u8]) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: protected.len() as u32,
        pbData: protected.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("Windows DPAPI failed to unprotect local cache bytes.".to_string());
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(bytes)
}

#[cfg(not(windows))]
pub fn unprotect_local_bytes(_protected: &[u8]) -> Result<Vec<u8>, String> {
    Err("Protected local cache bytes require Windows DPAPI in this phase.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_recovers_local_cache_bytes() {
        let input = b"project names and paths stay protected at rest";
        let protected = protect_local_bytes(input).expect("DPAPI should protect bytes");

        assert_ne!(protected, input);
        assert_eq!(
            unprotect_local_bytes(&protected).expect("DPAPI should recover bytes"),
            input
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn local_cache_protection_is_windows_only() {
        assert!(protect_local_bytes(b"cache").is_err());
        assert!(unprotect_local_bytes(b"cache").is_err());
    }
}
