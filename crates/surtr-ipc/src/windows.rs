//! Windows security for the IPC pipe: the current user's SID, and a
//! security descriptor that grants access to that user only.

#![allow(unsafe_code)]

use std::{ffi::c_void, io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, LocalFree},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The current user's SID, for example `S-1-5-21-...-1001`.
pub fn user_sid() -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: the pseudo-handle is always valid; `token` is an out-pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        let mut len = 0u32;
        // SAFETY: a null buffer with zero length asks for the size.
        unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len) };
        // u64 storage keeps TOKEN_USER aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` has `len` writable bytes.
        let ok = unsafe {
            GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len)
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: on success the buffer starts with a TOKEN_USER whose SID
        // points into the same buffer.
        let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text: *mut u16 = ptr::null_mut();
        // SAFETY: `sid` is valid while `buf` lives; `text` is an out-pointer.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `text` is a NUL-terminated string allocated by the call.
        let s = unsafe {
            let mut n = 0;
            while *text.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(text, n))
        };
        // SAFETY: allocated with LocalAlloc by ConvertSidToStringSidW.
        unsafe { LocalFree(text.cast()) };
        Ok(s)
    })();
    // SAFETY: `token` is open and closed once.
    unsafe { CloseHandle(token) };
    result
}

/// Security attributes whose access list allows only the current user.
pub struct OwnerOnly {
    descriptor: PSECURITY_DESCRIPTOR,
    attributes: SECURITY_ATTRIBUTES,
}

// SAFETY: the descriptor is immutable after creation and only read by
// CreateNamedPipe calls.
unsafe impl Send for OwnerOnly {}
// SAFETY: as above.
unsafe impl Sync for OwnerOnly {}

impl OwnerOnly {
    /// Builds `D:P(A;;GA;;;<user SID>)`: a protected DACL with one entry
    /// granting all access to the current user, and nothing to anyone else.
    pub fn new() -> io::Result<Self> {
        let sddl = wide(&format!("D:P(A;;GA;;;{})", user_sid()?));
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated; `descriptor` is an out-pointer.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            descriptor,
            attributes: SECURITY_ATTRIBUTES {
                nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            },
        })
    }

    /// The attributes, for `CreateNamedPipe`.
    pub fn attributes(&self) -> *mut c_void {
        (&raw const self.attributes).cast_mut().cast()
    }
}

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        // SAFETY: allocated with LocalAlloc by the conversion call; freed once.
        unsafe { LocalFree(self.descriptor) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_user_sid() {
        let sid = user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
    }

    #[test]
    fn owner_only_descriptor_builds() {
        let d = OwnerOnly::new().unwrap();
        assert!(!d.attributes().is_null());
    }
}
