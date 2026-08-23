//! SCM (service control manager) bindings — part of the FFI boundary
//! module family. Same policy as `ffi.rs`: the only place where `unsafe`
//! is permitted; every call wrapped in a safe, documented function.

#![allow(unsafe_code)]

use crate::ffi::{last_error, wide};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, DeleteService, EnumServicesStatusExW,
    OpenSCManagerW, OpenServiceW, SC_ENUM_PROCESS_INFO, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SERVICE_CONTROL_STOP, SERVICE_DISABLED, SERVICE_NO_CHANGE,
    SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_WIN32,
};

const SERVICE_QUERY_STATUS: u32 = 0x0000_0004;
const SERVICE_CHANGE_CONFIG: u32 = 0x0000_0002;
const DELETE_RIGHT: u32 = 0x0001_0000;

pub type Win32Error = u32;

const SERVICE_STOP: u32 = 0x0000_0010;

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

impl ScmGuard {
    /// Open with STOP | QUERY_STATUS | CHANGE_CONFIG | DELETE rights.
    pub fn open_service_full(&self, name: &str) -> Result<ServiceHandle, Win32Error> {
        let w = wide(name);
        // SAFETY: name outlives the call; handle closed by ServiceHandle::drop.
        let h = unsafe {
            OpenServiceW(
                self.0,
                w.as_ptr(),
                SERVICE_STOP | SERVICE_QUERY_STATUS | SERVICE_CHANGE_CONFIG | DELETE_RIGHT,
            )
        };
        if h.is_null() {
            Err(last_error())
        } else {
            Ok(ServiceHandle(h))
        }
    }
}

impl ServiceHandle {
    pub fn stop(&self) -> Result<(), Win32Error> {
        let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        // SAFETY: status is an out-parameter owned here.
        if unsafe { ControlService(self.0, SERVICE_CONTROL_STOP, &mut status) } != 0 {
            Ok(())
        } else {
            Err(last_error())
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
