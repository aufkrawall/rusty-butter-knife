//! Process identity/termination and UAC-cancellation bindings — part of the
//! FFI boundary family.
//!
//! Destructive process termination verifies the live image through the SAME
//! handle used to terminate it. This closes basename false positives and the
//! PID-reuse gap between a Toolhelp snapshot and OpenProcess.

#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenEventW, OpenProcess, QueryFullProcessImageNameW, SetEvent, TerminateProcess,
    WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

const WAIT_OBJECT_0: u32 = 0;

const KNOWN_KILLABLE_PROCESS_NAMES: &[&str] = &[
    "nvtelemetrycontainer.exe",
    "nvidia share.exe",
    "nvsphelper64.exe",
    "nvidia web helper.exe",
    "nvidia app.exe",
    "nvbackend.exe",
    "nvnodejslauncher.exe",
    "nvidia overlay.exe",
    "nvidia broadcast.exe",
    "nvstreamservice.exe",
    "nvstreamnetworkservice.exe",
    "nvstreamuseragent.exe",
    "frameview.exe",
    "presentmon.exe",
];

/// Only explicit historical process basenames are eligible for termination.
/// Generic "telemetry"/"frameview" substring matches are deliberately not
/// accepted at the mutation boundary.
pub fn process_name_is_explicitly_killable(exe_name: &str) -> bool {
    KNOWN_KILLABLE_PROCESS_NAMES
        .iter()
        .any(|n| n.eq_ignore_ascii_case(exe_name))
}

fn image_leaf(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Require both the expected executable basename and a known NVIDIA-owned
/// installation root. A user-created directory merely named "NVIDIA
/// Corporation" elsewhere on disk does not qualify. When uncertain, fail
/// closed and leave the process running.
pub fn process_image_is_nvidia_owned(path: &str, expected_exe_name: &str) -> bool {
    if !image_leaf(path).eq_ignore_ascii_case(expected_exe_name) {
        return false;
    }
    let l = path.to_lowercase();
    l.contains(":\\program files\\nvidia corporation\\")
        || l.contains(":\\program files (x86)\\nvidia corporation\\")
        || l.contains(":\\programdata\\nvidia corporation\\")
        || l.contains(":\\programdata\\nvidia\\")
        || l.contains("\\windows\\system32\\driverstore\\filerepository\\nv")
}

/// Service/task mutation requires a stronger NVIDIA anchor than an arbitrary
/// alphanumeric token beginning with "nv". This prevents combinations such
/// as an unrelated `NvBackup Update` target from being classified by the broad
/// semantic `update` term.
pub fn has_strong_nvidia_named_target_context(text: &str) -> bool {
    let l = text.to_lowercase();
    if l.contains("nvidia") || l.contains("geforce") || l.contains("displaydriverras") {
        return true;
    }
    const PREFIXES: &[&str] = &[
        "nvtelemetry",
        "nvtm",
        "nvprofile",
        "nvdriver",
        "nvota",
        "nvbackend",
        "nvmodule",
        "nvnode",
        "nvstream",
        "nvvad",
        "nvwmi",
        "nvcamera",
    ];
    l.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .any(|t| PREFIXES.iter().any(|p| t.starts_with(p)))
}

fn query_image_path(handle: HANDLE) -> Result<String, u32> {
    let mut buf = vec![0u16; 32768];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len) };
    if ok == 0 {
        return Err(crate::ffi::current_last_error());
    }
    buf.truncate(len as usize);
    Ok(String::from_utf16_lossy(&buf))
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn open_and_verify(
    pid: u32,
    rights: u32,
    expected_exe_name: &str,
) -> Result<(ProcessHandle, String), String> {
    let handle = unsafe { OpenProcess(rights | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err(format!(
            "OpenProcess failed: {}",
            crate::winfmt::format_win_error(crate::ffi::current_last_error())
        ));
    }
    let guard = ProcessHandle(handle);
    let image = query_image_path(guard.0).map_err(|e| {
        format!(
            "QueryFullProcessImageNameW failed: {}",
            crate::winfmt::format_win_error(e)
        )
    })?;
    if !process_image_is_nvidia_owned(&image, expected_exe_name) {
        return Err(format!("live image is not an approved NVIDIA target: {image}"));
    }
    Ok((guard, image))
}

/// Dry-run identity check using query-only rights.
pub fn verify_process_for_dry_run(pid: u32, expected_exe_name: &str) -> Result<String, String> {
    let (_guard, image) = open_and_verify(pid, 0, expected_exe_name)?;
    Ok(image)
}

/// Terminate-capable handle whose live image was verified through this exact
/// handle, eliminating the process-snapshot/PID-reuse race.
pub struct VerifiedTerminateHandle(ProcessHandle);

impl VerifiedTerminateHandle {
    pub fn open(pid: u32, expected_exe_name: &str) -> Result<(Self, String), String> {
        let (handle, image) = open_and_verify(
            pid,
            PROCESS_TERMINATE | SYNCHRONIZE,
            expected_exe_name,
        )?;
        Ok((Self(handle), image))
    }

    pub fn terminate(&self, exit_code: u32) -> bool {
        unsafe { TerminateProcess(self.0.0, exit_code) != 0 }
    }

    pub fn wait_ms(&self, ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.0.0, ms) == WAIT_OBJECT_0 }
    }
}

/// Manual-reset named event owned by the unelevated launcher while its UAC
/// child is running. The console handler itself remains atomic-only; normal
/// wait code signals this event after observing the local abort flag.
pub struct NamedAbortEvent(HANDLE);

impl NamedAbortEvent {
    pub fn create(name: &str) -> Option<Self> {
        let wide = crate::ffi::wide(name);
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, wide.as_ptr()) };
        if handle.is_null() {
            None
        } else {
            Some(Self(handle))
        }
    }

    pub fn signal(&self) -> bool {
        unsafe { SetEvent(self.0) != 0 }
    }
}

impl Drop for NamedAbortEvent {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Observe a named abort event without retaining a process-global kernel
/// handle. Missing/inaccessible events fail closed toward "not aborted"; the
/// elevated child continues unless its own local Ctrl+C flag is also set.
pub fn named_abort_event_is_signaled(name: &str) -> bool {
    let wide = crate::ffi::wide(name);
    let handle = unsafe { OpenEventW(SYNCHRONIZE, 0, wide.as_ptr()) };
    if handle.is_null() {
        return false;
    }
    let signaled = unsafe { WaitForSingleObject(handle, 0) == WAIT_OBJECT_0 };
    unsafe { CloseHandle(handle) };
    signaled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_name_allowlist_has_no_generic_substring_fallback() {
        assert!(process_name_is_explicitly_killable("NvTelemetryContainer.exe"));
        assert!(process_name_is_explicitly_killable("PresentMon.exe"));
        assert!(!process_name_is_explicitly_killable("TelemetryAgent.exe"));
        assert!(!process_name_is_explicitly_killable("MyFrameViewTool.exe"));
    }

    #[test]
    fn process_image_requires_matching_leaf_and_known_nvidia_root() {
        assert!(process_image_is_nvidia_owned(
            r"C:\Program Files\NVIDIA Corporation\FrameViewSDK\PresentMon.exe",
            "presentmon.exe"
        ));
        assert!(process_image_is_nvidia_owned(
            r"D:\ProgramData\NVIDIA Corporation\Telemetry\NvTelemetryContainer.exe",
            "NvTelemetryContainer.exe"
        ));
        assert!(!process_image_is_nvidia_owned(
            r"C:\Tools\PresentMon.exe",
            "presentmon.exe"
        ));
        assert!(!process_image_is_nvidia_owned(
            r"C:\Malware\NVIDIA Corporation\PresentMon.exe",
            "presentmon.exe"
        ));
        assert!(!process_image_is_nvidia_owned(
            r"C:\Program Files\NVIDIA Corporation\FrameViewSDK\Other.exe",
            "presentmon.exe"
        ));
    }

    #[test]
    fn service_task_context_rejects_arbitrary_nv_plus_generic_term() {
        assert!(has_strong_nvidia_named_target_context(
            "NvProfileUpdater NVIDIA Profile Updater"
        ));
        assert!(has_strong_nvidia_named_target_context(
            r"\NvTmRep\NVIDIA Telemetry Task"
        ));
        assert!(!has_strong_nvidia_named_target_context("NvBackup Update"));
        assert!(!has_strong_nvidia_named_target_context(r"\NvCache\Share cleanup"));
    }

    #[test]
    fn named_abort_event_round_trip() {
        let name = format!(
            r"Local\RustyButterKnife_Abort_{}",
            crate::util::now_unique_suffix()
        );
        let event = NamedAbortEvent::create(&name).expect("create named abort event");
        assert!(!named_abort_event_is_signaled(&name));
        assert!(event.signal());
        assert!(named_abort_event_is_signaled(&name));
    }
}
