//! Read-only lock diagnostics — part of the FFI boundary module family.
//! Restart Manager lookups name the processes holding a file that could not
//! be deleted, and a registry read exposes reboot-pending file operations.
//! Nothing here mutates system state: Restart Manager sessions only register
//! resources and list holders (RmShutdown/RmRestart are never called).

#![allow(unsafe_code)]

use crate::ffi::wide;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_MULTI_SZ};
use windows_sys::Win32::System::RestartManager::{
    RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
    RM_PROCESS_INFO,
};

pub type Win32Error = u32;

/// Bounded retries for the size-then-fill call pairs below: the holder list
/// or registry value can grow between the two calls, but never indefinitely.
const SIZE_RACE_RETRIES: usize = 3;

/// One process that Restart Manager reports as using a file.
#[derive(Debug, Clone)]
pub struct LockHolder {
    pub pid: u32,
    /// Friendly application name (service display name for services).
    pub app_name: String,
    /// Service short name; empty for non-service processes.
    pub service_name: String,
}

/// Restart Manager session handle; ended exactly once on Drop.
struct RmSession(u32);

impl Drop for RmSession {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RmStartSession and is
        // ended exactly once here.
        unsafe { RmEndSession(self.0) };
    }
}

fn utf16_until_nul(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Processes currently holding `path` open (or mapped as a module), per the
/// documented Restart Manager API.
pub fn query_file_lock_holders(path: &str) -> Result<Vec<LockHolder>, Win32Error> {
    let mut handle: u32 = 0;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: out-pointers reference live locals; the key buffer has the
    // documented CCH_RM_SESSION_KEY + 1 capacity.
    let rc = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
    if rc != ERROR_SUCCESS {
        return Err(rc);
    }
    let session = RmSession(handle);

    let wpath = wide(path);
    let files = [wpath.as_ptr()];
    // SAFETY: one NUL-terminated path that outlives the call; no
    // applications or services are registered.
    let rc = unsafe {
        RmRegisterResources(
            session.0,
            1,
            files.as_ptr(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(rc);
    }

    let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
    for _ in 0..SIZE_RACE_RETRIES {
        let mut needed: u32 = 0;
        let mut count: u32 = infos.len() as u32;
        let mut reasons: u32 = 0;
        // SAFETY: `infos` holds `count` zero-initialized RM_PROCESS_INFO
        // slots (or the pointer is null with count 0 for the size probe).
        let rc = unsafe {
            RmGetList(
                session.0,
                &mut needed,
                &mut count,
                if infos.is_empty() {
                    std::ptr::null_mut()
                } else {
                    infos.as_mut_ptr()
                },
                &mut reasons,
            )
        };
        if rc == ERROR_SUCCESS {
            infos.truncate(count as usize);
            return Ok(infos
                .iter()
                .map(|i| LockHolder {
                    pid: i.Process.dwProcessId,
                    app_name: utf16_until_nul(&i.strAppName),
                    service_name: utf16_until_nul(&i.strServiceShortName),
                })
                .collect());
        }
        if rc != ERROR_MORE_DATA {
            return Err(rc);
        }
        // SAFETY: RM_PROCESS_INFO is plain data; all-zero is a valid value.
        infos = vec![unsafe { std::mem::zeroed::<RM_PROCESS_INFO>() }; needed as usize];
    }
    Err(ERROR_MORE_DATA)
}

/// Raw REG_MULTI_SZ data (UTF-16 units, terminators included) of one value
/// below HKLM; `Ok(None)` when the value does not exist.
pub fn read_hklm_multi_sz(subkey: &str, value: &str) -> Result<Option<Vec<u16>>, Win32Error> {
    let wsub = wide(subkey);
    let wval = wide(value);
    let mut cb: u32 = 0;
    for _ in 0..SIZE_RACE_RETRIES {
        let mut buf = vec![0u16; (cb as usize).div_ceil(2)];
        let mut got = cb;
        // SAFETY: strings are NUL-terminated and outlive the call; `buf`
        // has `got` bytes of capacity (null data pointer for the size probe).
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                wsub.as_ptr(),
                wval.as_ptr(),
                RRF_RT_REG_MULTI_SZ,
                std::ptr::null_mut(),
                if buf.is_empty() {
                    std::ptr::null_mut()
                } else {
                    buf.as_mut_ptr().cast()
                },
                &mut got,
            )
        };
        if rc == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if rc == ERROR_SUCCESS && !buf.is_empty() {
            buf.truncate((got as usize) / 2);
            return Ok(Some(buf));
        }
        if rc == ERROR_SUCCESS && got == 0 {
            return Ok(Some(Vec::new()));
        }
        if rc != ERROR_SUCCESS && rc != ERROR_MORE_DATA {
            return Err(rc);
        }
        cb = got; // size probe or growth race: retry with the reported size
    }
    Err(ERROR_MORE_DATA)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_manager_reports_own_open_handle() {
        use std::os::windows::fs::OpenOptionsExt;
        let p = std::env::temp_dir().join(format!(
            "gpd-rm-{}-{}.txt",
            std::process::id(),
            crate::util::now_unique_suffix()
        ));
        std::fs::write(&p, b"x").unwrap();
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&p)
            .unwrap();
        let holders = query_file_lock_holders(&p.to_string_lossy()).expect("RmGetList");
        drop(handle);
        let _ = std::fs::remove_file(&p);
        let me = std::process::id();
        assert!(
            holders.iter().any(|h| h.pid == me),
            "own PID {me} not reported: {holders:?}"
        );
    }

    #[test]
    fn missing_registry_value_reads_as_none() {
        let got = read_hklm_multi_sz(
            "SYSTEM\\CurrentControlSet\\Control\\Session Manager",
            "RustyButterKnifeNoSuchValue",
        );
        assert_eq!(got, Ok(None));
    }
}
