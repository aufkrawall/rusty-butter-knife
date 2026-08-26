//! SCM (service control manager) bindings — part of the FFI boundary
//! module family. Same policy as `ffi.rs`: the only place where `unsafe`
//! is permitted; every call wrapped in a safe, documented function.

#![allow(unsafe_code)]

use crate::ffi::{last_error, wide};
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, DeleteService, EnumServicesStatusExW,
    OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_ENUM_PROCESS_INFO, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SC_STATUS_PROCESS_INFO, SERVICE_CHANGE_CONFIG,
    SERVICE_CONTROL_STOP, SERVICE_DISABLED, SERVICE_NO_CHANGE, SERVICE_QUERY_STATUS,
    SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_STATUS_PROCESS, SERVICE_STOP, SERVICE_STOPPED,
    SERVICE_WIN32,
};

pub type Win32Error = u32;

/// OpenService DELETE access right (generic right 0x00010000); the Services
/// metadata module does not re-export it under a dedicated name.
const SERVICE_DELETE_RIGHT: u32 = 0x0001_0000;

pub struct ScmGuard(windows_sys::Win32::System::Services::SC_HANDLE);

pub fn open_service_control_manager() -> Result<ScmGuard, Win32Error> {
    // SAFETY: handle closed by ScmGuard::drop.
    let h = unsafe {
        OpenSCManagerW(
            std::ptr::null(),
            std::ptr::null(),
            SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT,
        )
    };
    if h.is_null() {
        Err(last_error())
    } else {
        Ok(ScmGuard(h))
    }
}

impl Drop for ScmGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseServiceHandle(self.0) };
        }
    }
}

#[derive(Clone)]
pub struct ServiceEntry {
    pub name: String,
    pub display: String,
}

/// Enumerate WIN32 services in any state (port of EnumServicesStatusExW usage).
pub fn enumerate_win32_services() -> Result<Vec<ServiceEntry>, Win32Error> {
    let scm = open_service_control_manager()?;
    let mut bytes_needed = 0u32;
    let mut count = 0u32;
    let mut resume = 0u32;
    unsafe {
        EnumServicesStatusExW(
            scm.0,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            std::ptr::null_mut(),
            0,
            &mut bytes_needed,
            &mut count,
            &mut resume,
            std::ptr::null(),
        )
    };
    let mut buf = vec![0u8; bytes_needed as usize + 4096];
    let ok = unsafe {
        EnumServicesStatusExW(
            scm.0,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut bytes_needed,
            &mut count,
            &mut resume,
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(last_error());
    }
    let stride =
        std::mem::size_of::<windows_sys::Win32::System::Services::ENUM_SERVICE_STATUS_PROCESSW>();
    let base = buf.as_mut_ptr();
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        // SAFETY: entries form a contiguous OS-populated array inside `buf`.
        let entry = unsafe {
            base.add(i * stride)
                .cast::<windows_sys::Win32::System::Services::ENUM_SERVICE_STATUS_PROCESSW>()
        };
        let e = unsafe { entry.read_unaligned() };
        let pw = |p: *mut u16| -> String {
            if p.is_null() {
                return String::new();
            }
            let mut end = 0usize;
            unsafe {
                while *p.add(end) != 0 {
                    end += 1;
                }
                String::from_utf16_lossy(std::slice::from_raw_parts(p, end))
            }
        };
        out.push(ServiceEntry {
            name: pw(e.lpServiceName),
            display: pw(e.lpDisplayName),
        });
    }
    Ok(out)
}

pub struct ServiceHandle(windows_sys::Win32::System::Services::SC_HANDLE);

/// Typed outcome of a stop-with-convergence request (audit finding
/// SERVICE-02): stopped / already stopped / timeout / query failure are
/// distinct results instead of one opaque success/failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// Confirmed stopped via QueryServiceStatusEx.
    Stopped,
    /// Confirmed already stopped before the control request.
    AlreadyStopped,
    /// Still not stopped after the convergence deadline.
    StopPendingTimeout,
    /// Status could not be read; the stop itself may have succeeded.
    QueryFailed(Win32Error),
}

/// Pure decision core for least-privilege service access (audit finding
/// SERVICE-01): request only what the requested operations exercise. Unit
/// tests below pin the exact masks.
pub fn desired_access_for_ops(
    stop_needed: bool,
    change_config_needed: bool,
    delete_needed: bool,
) -> u32 {
    let mut desired_access = SERVICE_QUERY_STATUS;
    if stop_needed {
        desired_access |= SERVICE_STOP;
    }
    if change_config_needed {
        desired_access |= SERVICE_CHANGE_CONFIG;
    }
    if delete_needed {
        desired_access |= SERVICE_DELETE_RIGHT;
    }
    desired_access
}

impl ScmGuard {
    /// Open with the least privileges the requested operations need.
    /// Requesting only what we exercise avoids OpenService failing outright
    /// on services whose DACL denies rights we never use (e.g. DELETE).
    pub fn open_service_for_ops(
        &self,
        name: &str,
        stop_needed: bool,
        change_config_needed: bool,
        delete_needed: bool,
    ) -> Result<ServiceHandle, Win32Error> {
        let desired_access =
            desired_access_for_ops(stop_needed, change_config_needed, delete_needed);
        let w = wide(name);
        // SAFETY: name outlives the call; handle closed by ServiceHandle::drop.
        let h = unsafe { OpenServiceW(self.0, w.as_ptr(), desired_access) };
        if h.is_null() {
            Err(last_error())
        } else {
            Ok(ServiceHandle(h))
        }
    }
}

impl ServiceHandle {
    /// Request SERVICE_CONTROL_STOP and then OBSERVE convergence within a
    /// bounded window (audit finding SERVICE-02): a still-exiting service can
    /// hold files that later stages try to remove.
    pub fn stop(&self) -> Result<StopOutcome, Win32Error> {
        // Observe-before-control: an already-stopped service must not surface
        // as an ERROR_SERVICE_NOT_ACTIVE failure.
        if let Ok(StopOutcome::Stopped) = self.wait_until_stopped(0) {
            return Ok(StopOutcome::AlreadyStopped);
        }
        // SAFETY: status out-parameter owned here; handle valid in guard.
        let ok = unsafe {
            let mut status: SERVICE_STATUS = std::mem::zeroed();
            ControlService(self.0, SERVICE_CONTROL_STOP, &mut status)
        } != 0;
        if !ok {
            return Err(last_error());
        }
        self.wait_until_stopped(10_000)
    }

    /// Poll QueryServiceStatusEx until STOPPED or the deadline elapses. A
    /// zero timeout performs a single observation without waiting.
    pub fn wait_until_stopped(&self, timeout_ms: u32) -> Result<StopOutcome, Win32Error> {
        let start = Instant::now();
        loop {
            let mut proc_status: SERVICE_STATUS_PROCESS = unsafe { std::mem::zeroed() };
            let mut needed = 0u32;
            // SAFETY: fixed-size out-buffer per the QueryServiceStatusEx contract.
            let ok = unsafe {
                QueryServiceStatusEx(
                    self.0,
                    SC_STATUS_PROCESS_INFO,
                    (&mut proc_status as *mut SERVICE_STATUS_PROCESS).cast(),
                    std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32,
                    &mut needed,
                )
            } != 0;
            if !ok {
                return Ok(StopOutcome::QueryFailed(last_error()));
            }
            let elapsed = start.elapsed();
            if proc_status.dwCurrentState == SERVICE_STOPPED {
                return Ok(StopOutcome::Stopped);
            }
            if timeout_ms == 0 || elapsed >= Duration::from_millis(u64::from(timeout_ms)) {
                return Ok(StopOutcome::StopPendingTimeout);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    pub fn disable(&self) -> Result<(), Win32Error> {
        // SAFETY: unchanged parameters passed as NO_CHANGE/null per contract.
        if unsafe {
            ChangeServiceConfigW(
                self.0,
                SERVICE_NO_CHANGE,
                SERVICE_DISABLED,
                SERVICE_NO_CHANGE,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        } != 0
        {
            Ok(())
        } else {
            Err(last_error())
        }
    }

    pub fn delete(&self) -> Result<(), Win32Error> {
        // SAFETY: handle valid while the guard is alive.
        if unsafe { DeleteService(self.0) } != 0 {
            Ok(())
        } else {
            Err(last_error())
        }
    }
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseServiceHandle(self.0) };
        }
    }
}

/// Unit tests pinning exact access masks for the least-privilege split
/// (audit finding SERVICE-01).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_status_is_always_requested() {
        assert_eq!(
            desired_access_for_ops(false, false, false),
            SERVICE_QUERY_STATUS
        );
    }

    #[test]
    fn disable_only_needs_change_config_without_delete() {
        let mask = desired_access_for_ops(false, true, false);
        assert_eq!(mask, SERVICE_QUERY_STATUS | SERVICE_CHANGE_CONFIG);
        assert_eq!(
            mask & SERVICE_DELETE_RIGHT,
            0,
            "disable-only must NOT require DELETE"
        );
        assert_eq!(mask & SERVICE_STOP, 0);
    }

    #[test]
    fn delete_only_needs_delete_without_change_config() {
        let mask = desired_access_for_ops(false, false, true);
        assert_eq!(mask, SERVICE_QUERY_STATUS | SERVICE_DELETE_RIGHT);
        assert_eq!(
            mask & SERVICE_CHANGE_CONFIG,
            0,
            "delete-only must NOT require CHANGE_CONFIG"
        );
    }

    #[test]
    fn stop_only_needs_stop_and_query() {
        let mask = desired_access_for_ops(true, false, false);
        assert_eq!(mask, SERVICE_QUERY_STATUS | SERVICE_STOP);
        assert_eq!(mask & SERVICE_DELETE_RIGHT, 0);
        assert_eq!(mask & SERVICE_CHANGE_CONFIG, 0);
    }

    #[test]
    fn full_stack_combines_independent_bits() {
        let mask = desired_access_for_ops(true, true, true);
        assert_eq!(
            mask,
            SERVICE_QUERY_STATUS | SERVICE_STOP | SERVICE_CHANGE_CONFIG | SERVICE_DELETE_RIGHT
        );
    }
}
