//! Windows privilege gate for the one-shot object-backup helper.
//!
//! Enabling privileges is not evidence by itself: callers must still open every
//! object through the bound/no-follow primitives and perform a complete
//! BackupRead -> BackupWrite -> BackupRead round trip. This module merely makes
//! it impossible to label a result complete when the required token rights are
//! absent.

use serde::{Deserialize, Serialize};

pub const PRIVILEGE_BACKUP: u32 = 1 << 0;
pub const PRIVILEGE_RESTORE: u32 = 1 << 1;
pub const PRIVILEGE_SECURITY: u32 = 1 << 2;
pub const REQUIRED_OBJECT_PRIVILEGES: u32 =
    PRIVILEGE_BACKUP | PRIVILEGE_RESTORE | PRIVILEGE_SECURITY;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivilegeProof {
    pub elevated: bool,
    pub high_integrity: bool,
    pub enabled_bitmap: u32,
}

impl PrivilegeProof {
    pub fn require_object_complete(self) -> Result<Self, String> {
        if self.elevated && self.high_integrity && self.enabled_bitmap == REQUIRED_OBJECT_PRIVILEGES
        {
            Ok(self)
        } else {
            Err(format!(
                "elevated helper lacks the complete backup/restore/security privilege proof (elevated={}, high_integrity={}, bitmap=0x{:x})",
                self.elevated, self.high_integrity, self.enabled_bitmap
            ))
        }
    }
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct ThreePrivileges {
    count: u32,
    values: [windows_sys::Win32::Security::LUID_AND_ATTRIBUTES; 3],
}

/// Keeps the three narrowly-scoped privileges enabled only while object
/// capture/replay is in progress and restores their exact prior token state on
/// every return path.
pub struct PrivilegeGuard {
    proof: PrivilegeProof,
    #[cfg(windows)]
    token: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    previous: ThreePrivileges,
}

impl PrivilegeGuard {
    pub fn proof(&self) -> PrivilegeProof {
        self.proof
    }
}

#[cfg(windows)]
impl Drop for PrivilegeGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::Security::AdjustTokenPrivileges;
        unsafe {
            let _ = AdjustTokenPrivileges(
                self.token,
                0,
                (&self.previous as *const ThreePrivileges).cast(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            CloseHandle(self.token);
        }
    }
}

#[cfg(windows)]
pub fn enable_object_backup_privileges() -> Result<PrivilegeGuard, String> {
    use windows_sys::Win32::Foundation::ERROR_NOT_ALL_ASSIGNED;
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation,
        LookupPrivilegeValueW, TokenElevation, TokenElevationType, TokenElevationTypeFull,
        TokenIntegrityLevel, TokenPrivileges, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE, TOKEN_MANDATORY_LABEL,
        TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut raw = std::ptr::null_mut();
    let opened = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
            &mut raw,
        )
    };
    if opened == 0 {
        return Err(format!(
            "cannot open elevated helper token: {}",
            std::io::Error::last_os_error()
        ));
    }
    struct PendingToken(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for PendingToken {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    windows_sys::Win32::Foundation::CloseHandle(self.0);
                }
            }
        }
    }
    let mut token = PendingToken(raw);

    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    let elevation_ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    if elevation_ok == 0 {
        return Err(format!(
            "cannot prove helper token elevation: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut elevation_type: TOKEN_ELEVATION_TYPE = 0;
    let elevation_type_ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevationType,
            (&mut elevation_type as *mut TOKEN_ELEVATION_TYPE).cast(),
            std::mem::size_of::<TOKEN_ELEVATION_TYPE>() as u32,
            &mut returned,
        )
    };
    if elevation_type_ok == 0 || elevation_type != TokenElevationTypeFull {
        return Err("object helper token is not TokenElevationTypeFull".to_string());
    }

    let mut integrity_bytes = 0u32;
    unsafe {
        GetTokenInformation(
            token.0,
            TokenIntegrityLevel,
            std::ptr::null_mut(),
            0,
            &mut integrity_bytes,
        );
    }
    if integrity_bytes < std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32
        || integrity_bytes > 64 * 1024
    {
        return Err("cannot bound helper integrity-token information".to_string());
    }
    let words = (integrity_bytes as usize).div_ceil(std::mem::size_of::<usize>());
    let mut integrity_storage = vec![0usize; words];
    let integrity_ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenIntegrityLevel,
            integrity_storage.as_mut_ptr().cast(),
            integrity_bytes,
            &mut returned,
        )
    };
    if integrity_ok == 0 {
        return Err(format!(
            "cannot prove helper integrity level: {}",
            std::io::Error::last_os_error()
        ));
    }
    let label = unsafe { &*(integrity_storage.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()) };
    if label.Label.Sid.is_null() {
        return Err("helper integrity SID is missing".to_string());
    }
    let sub_count = unsafe { *GetSidSubAuthorityCount(label.Label.Sid) as u32 };
    if sub_count == 0 {
        return Err("helper integrity SID has no RID".to_string());
    }
    let integrity_rid = unsafe { *GetSidSubAuthority(label.Label.Sid, sub_count - 1) };
    const SECURITY_MANDATORY_HIGH_RID: u32 = 0x0000_3000;
    let high_integrity = integrity_rid >= SECURITY_MANDATORY_HIGH_RID;
    // Refuse a medium/filtered token before looking up or adjusting any
    // privilege. A post-adjust proof is still required below, but it must not be
    // the first point at which an ineligible helper is rejected.
    if elevation.TokenIsElevated == 0 || !high_integrity {
        return Err(format!(
            "object helper token is not elevated at high integrity (elevated={}, high_integrity={high_integrity})",
            elevation.TokenIsElevated != 0
        ));
    }

    let mut privileges = ThreePrivileges {
        count: 3,
        values: [LUID_AND_ATTRIBUTES::default(); 3],
    };
    for (slot, name) in privileges.values.iter_mut().zip([
        "SeBackupPrivilege",
        "SeRestorePrivilege",
        "SeSecurityPrivilege",
    ]) {
        let wide = name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let found =
            unsafe { LookupPrivilegeValueW(std::ptr::null(), wide.as_ptr(), &mut slot.Luid) };
        if found == 0 {
            return Err(format!(
                "cannot resolve {name}: {}",
                std::io::Error::last_os_error()
            ));
        }
        slot.Attributes = SE_PRIVILEGE_ENABLED;
    }
    // AdjustTokenPrivileges can return success while setting
    // ERROR_NOT_ALL_ASSIGNED. Clear last-error first and check it explicitly.
    unsafe {
        windows_sys::Win32::Foundation::SetLastError(0);
    }
    let mut previous = ThreePrivileges {
        count: 0,
        values: [LUID_AND_ATTRIBUTES::default(); 3],
    };
    let mut previous_bytes = 0u32;
    let adjusted = unsafe {
        AdjustTokenPrivileges(
            token.0,
            0,
            (&privileges as *const ThreePrivileges).cast(),
            std::mem::size_of::<ThreePrivileges>() as u32,
            (&mut previous as *mut ThreePrivileges).cast(),
            &mut previous_bytes,
        )
    };
    let adjust_error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    // AdjustTokenPrivileges may have changed a strict subset even when it
    // reports ERROR_NOT_ALL_ASSIGNED. Transfer ownership to the restoration
    // guard before interpreting either status so every post-adjust exit rolls
    // back exactly the privilege attributes returned in `previous`.
    let guard_token = token.0;
    token.0 = std::ptr::null_mut();
    let mut guard = PrivilegeGuard {
        proof: PrivilegeProof {
            elevated: elevation.TokenIsElevated != 0,
            high_integrity,
            enabled_bitmap: 0,
        },
        token: guard_token,
        previous,
    };
    if adjusted == 0 || adjust_error == ERROR_NOT_ALL_ASSIGNED {
        return Err(format!(
            "required helper privileges were not assigned: {}",
            std::io::Error::from_raw_os_error(adjust_error as i32)
        ));
    }
    if adjust_error != 0 {
        return Err(format!(
            "cannot enable helper privileges: {}",
            std::io::Error::from_raw_os_error(adjust_error as i32)
        ));
    }

    let mut privilege_bytes = 0u32;
    unsafe {
        GetTokenInformation(
            guard.token,
            TokenPrivileges,
            std::ptr::null_mut(),
            0,
            &mut privilege_bytes,
        );
    }
    if privilege_bytes < std::mem::size_of::<TOKEN_PRIVILEGES>() as u32
        || privilege_bytes > 1024 * 1024
    {
        return Err("cannot bound helper TokenPrivileges result".to_string());
    }
    let words = (privilege_bytes as usize).div_ceil(std::mem::size_of::<usize>());
    let mut privilege_storage = vec![0usize; words];
    let privileges_ok = unsafe {
        GetTokenInformation(
            guard.token,
            TokenPrivileges,
            privilege_storage.as_mut_ptr().cast(),
            privilege_bytes,
            &mut returned,
        )
    };
    if privileges_ok == 0 {
        return Err(format!(
            "cannot prove enabled helper privileges: {}",
            std::io::Error::last_os_error()
        ));
    }
    let listed = unsafe { &*(privilege_storage.as_ptr().cast::<TOKEN_PRIVILEGES>()) };
    let count = listed.PrivilegeCount as usize;
    let base = unsafe {
        privilege_storage
            .as_ptr()
            .cast::<u8>()
            .add(std::mem::offset_of!(TOKEN_PRIVILEGES, Privileges))
            .cast::<LUID_AND_ATTRIBUTES>()
    };
    let needed = std::mem::offset_of!(TOKEN_PRIVILEGES, Privileges)
        .checked_add(count.saturating_mul(std::mem::size_of::<LUID_AND_ATTRIBUTES>()))
        .ok_or_else(|| "TokenPrivileges size overflow".to_string())?;
    if needed > privilege_bytes as usize {
        return Err("TokenPrivileges returned invalid bounds".to_string());
    }
    let listed = unsafe { std::slice::from_raw_parts(base, count) };
    let mut bitmap = 0u32;
    for (index, target) in privileges.values.iter().enumerate() {
        if listed.iter().any(|entry| {
            entry.Luid.LowPart == target.Luid.LowPart
                && entry.Luid.HighPart == target.Luid.HighPart
                && entry.Attributes & SE_PRIVILEGE_ENABLED != 0
        }) {
            bitmap |= 1 << index;
        }
    }
    guard.proof.enabled_bitmap = bitmap;
    guard.proof = PrivilegeProof {
        elevated: elevation.TokenIsElevated != 0,
        high_integrity,
        enabled_bitmap: bitmap,
    }
    .require_object_complete()?;
    Ok(guard)
}

#[cfg(not(windows))]
pub fn enable_object_backup_privileges() -> Result<PrivilegeGuard, String> {
    Err("the elevated object-backup helper is Windows-only".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_or_medium_token_can_never_claim_object_complete() {
        for proof in [
            PrivilegeProof {
                elevated: false,
                high_integrity: false,
                enabled_bitmap: 0,
            },
            PrivilegeProof {
                elevated: true,
                high_integrity: false,
                enabled_bitmap: REQUIRED_OBJECT_PRIVILEGES,
            },
            PrivilegeProof {
                elevated: true,
                high_integrity: true,
                enabled_bitmap: PRIVILEGE_BACKUP | PRIVILEGE_RESTORE,
            },
        ] {
            assert!(proof.require_object_complete().is_err());
        }
        assert!(PrivilegeProof {
            elevated: true,
            high_integrity: true,
            enabled_bitmap: REQUIRED_OBJECT_PRIVILEGES,
        }
        .require_object_complete()
        .is_ok());
    }
}
