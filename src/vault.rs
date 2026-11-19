use crate::api::Result;
use windows_sys::Win32::{
    Foundation::{ERROR_NOT_FOUND, GetLastError},
    Security::Credentials::*,
};
fn target() -> Result<Vec<u16>> {
    let scope = std::env::var("LAZYBUDDY_CREDENTIAL_SCOPE")
        .ok()
        .or_else(|| {
            std::env::var("LAZYBUDDY_INSTANCE")
                .ok()
                .filter(|s| s != "default")
        })
        .unwrap_or_else(|| "production".into());
    if scope.is_empty()
        || scope.len() > 48
        || !scope
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("invalid_credential_scope");
    }
    Ok(format!("LazyBuddy/bilibili/{scope}")
        .encode_utf16()
        .chain(Some(0))
        .collect())
}
pub fn read() -> Result<Option<String>> {
    let t = target()?;
    let mut p = std::ptr::null_mut();
    unsafe {
        if CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) == 0 {
            return if GetLastError() == ERROR_NOT_FOUND {
                Ok(None)
            } else {
                Err("credential_read_failed")
            };
        }
        let result = if p.is_null()
            || (*p).CredentialBlobSize == 0
            || (*p).CredentialBlobSize > 4096
            || (*p).CredentialBlob.is_null()
        {
            Err("credential_read_failed")
        } else {
            String::from_utf8(
                std::slice::from_raw_parts((*p).CredentialBlob, (*p).CredentialBlobSize as usize)
                    .to_vec(),
            )
            .map(Some)
            .map_err(|_| "credential_read_failed")
        };
        CredFree(p.cast());
        result
    }
}
pub fn write(cookie: &str) -> Result<()> {
    if cookie.is_empty() || cookie.len() > 4096 || cookie.contains(['\r', '\n']) {
        return Err("credential_save_failed");
    }
    let mut t = target()?;
    let mut blob = cookie.as_bytes().to_vec();
    let mut c: CREDENTIALW = unsafe { std::mem::zeroed() };
    c.Type = CRED_TYPE_GENERIC;
    c.TargetName = t.as_mut_ptr();
    c.CredentialBlobSize = blob.len() as u32;
    c.CredentialBlob = blob.as_mut_ptr();
    c.Persist = CRED_PERSIST_LOCAL_MACHINE;
    if unsafe { CredWriteW(&c, 0) } == 0 {
        Err("credential_save_failed")
    } else {
        Ok(())
    }
}
pub fn delete() -> Result<()> {
    let t = target()?;
    if unsafe { CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0
        && unsafe { GetLastError() } != ERROR_NOT_FOUND
    {
        Err("credential_save_failed")
    } else {
        Ok(())
    }
}
