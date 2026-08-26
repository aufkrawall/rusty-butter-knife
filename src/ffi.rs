//! The ONLY module allowed to contain `unsafe` (the crate root denies it
//! everywhere else). Every Win32/COM call used by the program lives here
//! behind a safe wrapper with its invariant stated. Errors are returned as
//! values (Win32 last-error codes / HRESULTs), never silently swallowed.
//! Handles are owned by guard types that release them on Drop.

#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_ALREADY_EXISTS, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Globalization::{GetOEMCP, MultiByteToWideChar};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{
    AllocateAndInitializeSid, CheckTokenMembership, FreeSid, GetTokenInformation,
    LookupAccountSidW, TokenUser, PSID, SID_IDENTIFIER_AUTHORITY, TOKEN_QUERY,
};
use windows_sys::Win32::Storage::FileSystem::{
    DeleteFileW, MoveFileExW, SetFileAttributesW, FILE_ATTRIBUTE_NORMAL,
    MOVEFILE_DELAY_UNTIL_REBOOT, SYNCHRONIZE,
};
use windows_sys::Win32::System::Console::{
    GetConsoleMode, GetStdHandle, SetConsoleCtrlHandler, SetConsoleTextAttribute, WriteConsoleW,
    STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    FormatMessageW, FORMAT_MESSAGE_ALLOCATE_BUFFER, FORMAT_MESSAGE_FROM_SYSTEM,
    FORMAT_MESSAGE_IGNORE_INSERTS,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Process32FirstW, Process32NextW,
    MODULEENTRY32W, PROCESSENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::SystemInformation::{
    GetLocalTime, GetSystemDirectoryW, GetTickCount64,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetCurrentProcessId, GetExitCodeProcess, OpenProcess,
    OpenProcessToken, TerminateProcess, WaitForSingleObject, PROCESS_TERMINATE,
};
use windows_sys::Win32::UI::Shell::{
    ShellExecuteExW, SEE_MASK_FLAG_DDEWAIT, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS,
    SHELLEXECUTEINFOW,
};

pub type Win32Error = u32;

/// NUL-terminated UTF-16 for LPCWSTR parameters.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

pub(crate) fn last_error() -> Win32Error {
    unsafe { GetLastError() }
}

// ---------------------------------------------------------------------------
// Time / system info
// ---------------------------------------------------------------------------

/// (year, month, day, hour, minute, second) in local time.
#[allow(clippy::many_single_char_names)]
pub fn local_time_parts() -> (u16, u16, u16, u16, u16, u16) {
    let mut st = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
    unsafe { GetLocalTime(&mut st) };
    (
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond,
    )
}

/// Milliseconds since boot (port of GetTickCount64 usage).
#[allow(dead_code)]
pub fn tick_count_64() -> u64 {
    unsafe { GetTickCount64() }
}

/// Current process id (used in the TI task name).
pub fn current_process_id() -> u32 {
    unsafe { GetCurrentProcessId() }
}

/// Real System32 directory (never the WOW64 redirect).
pub fn system_directory() -> Option<String> {
    let mut buf = [0u16; 260];
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 || n as usize >= buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..n as usize]))
}

/// Full path of the current executable.
pub fn module_file_name() -> Option<String> {
    let mut buf = [0u16; 32768];
    let len =
        unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) };
    if len == 0 || len as usize >= buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

// ---------------------------------------------------------------------------
// Encoding / message formatting
// ---------------------------------------------------------------------------

pub fn get_oem_code_page() -> u32 {
    unsafe { GetOEMCP() }
}

pub fn mb_to_wide(code_page: u32, flags: u32, bytes: &[u8]) -> String {
    let cb = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
    let needed = unsafe {
        MultiByteToWideChar(
            code_page,
            flags,
            bytes.as_ptr(),
            cb,
            std::ptr::null_mut(),
            0,
        )
    };
    if needed <= 0 {
        return String::new();
    }
    let mut out = vec![0u16; needed as usize];
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            flags,
            bytes.as_ptr(),
            cb,
            out.as_mut_ptr(),
            needed,
        )
    };
    if written <= 0 {
        return String::new();
    }
    out.truncate(written as usize);
    String::from_utf16_lossy(&out)
}

/// Port of `formatWinError`'s FormatMessage core: trimmed system message.
pub fn format_message(err: Win32Error) -> Option<String> {
    const FLAGS: u32 =
        FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS;
    let mut msg_ptr: *mut u16 = std::ptr::null_mut();
    let len = unsafe {
        FormatMessageW(
            FLAGS,
            std::ptr::null(),
            err,
            0,
            (&mut msg_ptr as *mut *mut u16).cast(),
            0,
            std::ptr::null(),
        )
    };
    if len == 0 || msg_ptr.is_null() {
        return None;
    }
    let slice = unsafe { std::slice::from_raw_parts(msg_ptr, len as usize) };
    let text = String::from_utf16_lossy(slice);
    // SAFETY: buffer was allocated by FormatMessageW (ALLOCATE_BUFFER) and is
    // freed exactly once here.
    unsafe { LocalFree(msg_ptr.cast()) };
    Some(text)
}

// ---------------------------------------------------------------------------
// Console (isize handle surface keeps call sites simple)
// ---------------------------------------------------------------------------

pub fn stdout_handle() -> isize {
    (unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }) as isize
}

pub fn stderr_handle() -> isize {
    (unsafe { GetStdHandle(STD_ERROR_HANDLE) }) as isize
}

pub fn is_console(h: isize) -> bool {
    if h == 0 {
        return false;
    }
    let mut mode: u32 = 0;
    unsafe { GetConsoleMode(h as HANDLE, &mut mode) != 0 }
}

pub fn set_console_text_attribute(h: isize, attr: u16) -> bool {
    unsafe { SetConsoleTextAttribute(h as HANDLE, attr) != 0 }
}

/// Write UTF-16 text to a console handle.
pub fn write_console_w(h: isize, text16: &[u16]) -> bool {
    let mut written = 0u32;
    unsafe {
        WriteConsoleW(
            h as HANDLE,
            text16.as_ptr(),
            text16.len() as u32,
            &mut written,
            std::ptr::null(),
        ) != 0
    }
}

/// Install the console control (Ctrl+C/close) handler.
pub fn set_console_ctrl_handler(handler: extern "system" fn(u32) -> i32) -> bool {
    unsafe { SetConsoleCtrlHandler(Some(handler), 1) != 0 }
}

// ---------------------------------------------------------------------------
// Single-instance mutex
// ---------------------------------------------------------------------------

pub struct MutexGuard {
    handle: HANDLE,
}

impl MutexGuard {
    pub fn held(&self) -> bool {
        !self.handle.is_null()
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.handle
    }
}

impl Drop for MutexGuard {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

/// Create-or-open the global named mutex. `already_existed` mirrors
/// GetLastError()==ERROR_ALREADY_EXISTS semantics.
pub fn create_global_mutex(name: &str) -> (MutexGuard, bool) {
    let wname = wide(name);
    // SAFETY: name is NUL-terminated wide; the handle is released by
    // MutexGuard's Drop.
    let h = unsafe { CreateMutexW(std::ptr::null(), 1, wname.as_ptr()) };
    let existed = last_error() == ERROR_ALREADY_EXISTS && !h.is_null();
    (MutexGuard { handle: h }, existed)
}

const WAIT_OBJECT_0: u32 = 0;

/// Acquire `name` within `ms` and return a guard that must be dropped to
/// release; None on timeout/creation failure so callers can degrade.
pub fn try_acquire_named_mutex(name: &str, ms: u32) -> Option<MutexGuard> {
    let (guard, _) = create_global_mutex(name);
    if guard.held() {
        let res =
            // SAFETY: valid named-mutex handle owned by the returned guard.
            unsafe { WaitForSingleObject(guard.raw(), ms) };
        if res == WAIT_OBJECT_0 {
            return Some(guard);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Audit-log writability probe (fail-closed destructive runs)
// ---------------------------------------------------------------------------

/// Verify the run log can actually be opened for appending BEFORE any
/// destructive stage starts. Uses std to stay outside this module's Win32
/// surface; the file path comes from initialized run state.
pub fn ensure_log_writable(path: &std::path::Path) -> Result<(), String> {
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .map(|_| ())
        .map_err(|e| format!("run log {} is not writable: {}", path.display(), e))
}

// ---------------------------------------------------------------------------
// Privilege / identity checks
// ---------------------------------------------------------------------------

/// Port of `isAdmin`: membership in the builtin Administrators group.
pub fn is_admin() -> bool {
    const SECURITY_NT_AUTHORITY: [u8; 6] = [0, 0, 0, 0, 0, 5];
    const SECURITY_BUILTIN_DOMAIN_RID: u32 = 32;
    const DOMAIN_ALIAS_RID_ADMINS: u32 = 544;
    let authority = SID_IDENTIFIER_AUTHORITY {
        Value: SECURITY_NT_AUTHORITY,
    };
    let mut admin_group: PSID = std::ptr::null_mut();
    let ok = unsafe {
        AllocateAndInitializeSid(
            &authority,
            2,
            SECURITY_BUILTIN_DOMAIN_RID,
            DOMAIN_ALIAS_RID_ADMINS,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut admin_group,
        )
    };
    if ok == 0 {
        return false;
    }
    let mut is_member: i32 = 0;
    // SAFETY: admin_group was allocated above and freed here on all paths.
    unsafe {
        CheckTokenMembership(std::ptr::null_mut(), admin_group, &mut is_member);
        FreeSid(admin_group);
    }
    is_member != 0
}

/// Port of `currentTokenAccount`: "DOMAIN\name" or SID string fallback.
pub fn current_token_account() -> String {
    struct TokenHandle(HANDLE);
    impl Drop for TokenHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    // SAFETY: GetCurrentProcess returns the pseudo-handle; token closed via
    // TokenHandle on every path.
    let token = unsafe {
        let mut h: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut h) == 0 {
            return "<unknown>".to_string();
        }
        TokenHandle(h)
    };

    let mut needed = 0u32;
    unsafe {
        GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        return "<unknown>".to_string();
    }
    let mut buf = vec![0u8; needed as usize];
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return "<unknown>".to_string();
    }
    // TOKEN_USER layout: first member is the SID pointer into the buffer.
    let sid: PSID = unsafe { *(buf.as_ptr() as *const PSID) };

    let mut name = [0u16; 256];
    let mut domain = [0u16; 256];
    let mut name_len = name.len() as u32;
    let mut domain_len = domain.len() as u32;
    let mut use_enum: i32 = 0;
    let ok = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut use_enum,
        )
    };
    if ok == 0 {
        let mut sid_str: *mut u16 = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut sid_str) } != 0 && !sid_str.is_null() {
            let mut end = 0usize;
            unsafe {
                while *sid_str.add(end) != 0 {
                    end += 1;
                }
            }
            let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sid_str, end) });
            unsafe { LocalFree(sid_str.cast()) };
            return s;
        }
        return "<sid unknown>".to_string();
    }
    format!(
        "{}\\{}",
        String::from_utf16_lossy(&domain[..domain_len as usize]),
        String::from_utf16_lossy(&name[..name_len as usize])
    )
}

// ---------------------------------------------------------------------------
// Process snapshot / termination
// ---------------------------------------------------------------------------

pub struct ProcessEntry {
    pub pid: u32,
    pub exe_name: String,
}

fn snapshot_processes() -> Result<Vec<ProcessEntry>, Win32Error> {
    // SAFETY: snapshot handle closed before every return; entry structs are
    // zero-initialized with dwSize set per the Toolhelp contract.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE || snap.is_null() {
        return Err(last_error());
    }
    let mut out = Vec::new();
    unsafe {
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut pe) != 0 {
            loop {
                let end = pe
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(pe.szExeFile.len());
                out.push(ProcessEntry {
                    pid: pe.th32ProcessID,
                    exe_name: String::from_utf16_lossy(&pe.szExeFile[..end]),
                });
                if Process32NextW(snap, &mut pe) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    Ok(out)
}

pub fn enum_running_processes() -> Result<Vec<ProcessEntry>, Win32Error> {
    snapshot_processes()
}

/// Loaded modules of one process (port of MODULE32 snapshot usage).
pub fn enum_process_modules(pid: u32) -> Result<Vec<String>, Win32Error> {
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) };
    if snap == INVALID_HANDLE_VALUE || snap.is_null() {
        return Err(last_error());
    }
    let mut out = Vec::new();
    unsafe {
        let mut me: MODULEENTRY32W = std::mem::zeroed();
        me.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        if Module32FirstW(snap, &mut me) != 0 {
            loop {
                let end = me
                    .szExePath
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(me.szExePath.len());
                out.push(String::from_utf16_lossy(&me.szExePath[..end]));
                if Module32NextW(snap, &mut me) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    Ok(out)
}

/// Owned terminate-capable process handle (port of killLockerProcesses).
pub struct TerminateHandle(HANDLE);

impl TerminateHandle {
    pub fn open(pid: u32) -> Option<TerminateHandle> {
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        let rights = PROCESS_TERMINATE | SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION;
        // SAFETY: raw OpenProcess; the handle is owned by the returned guard.
        let h = unsafe { OpenProcess(rights, 0, pid) };
        if h.is_null() {
            None
        } else {
            Some(TerminateHandle(h))
        }
    }

    pub fn terminate(&self, exit_code: u32) -> bool {
        // SAFETY: handle valid while the guard is alive.
        unsafe { TerminateProcess(self.0, exit_code) != 0 }
    }

    /// True when the process exited within `ms` milliseconds.
    pub fn wait_ms(&self, ms: u32) -> bool {
        const WAIT_OBJECT_0: u32 = 0;
        unsafe { WaitForSingleObject(self.0, ms) == WAIT_OBJECT_0 }
    }

    pub fn last_error() -> Win32Error {
        last_error()
    }
}

impl Drop for TerminateHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

// ---------------------------------------------------------------------------
// UAC relaunch (ShellExecuteExW "runas")
// ---------------------------------------------------------------------------

pub struct RunAsChild {
    process: HANDLE,
}

impl RunAsChild {
    /// Abort-aware wait for the elevated child's exit code. Polls in short
    /// slices so a Ctrl+C request escapes the wait promptly instead of
    /// blocking forever on a stuck elevation dialog or child (audit finding:
    /// waits must be cancellation-aware). If the abort fires mid-wait, one
    /// last 5 s grace is given for graceful shutdown before abandoning;
    /// 0xFFFF_FFFD signals "abandoned".
    pub fn wait_exit_code_aborting(&self, abort_requested: impl Fn() -> bool) -> u32 {
        const WAIT_OBJECT_0: u32 = 0;
        const STILL_ACTIVE: u32 = 259;
        const ABANDONED: u32 = 0xFFFF_FFFD;
        loop {
            let mut rc = STILL_ACTIVE;
            let signaled =
                // SAFETY: owned child process handle from ShellExecuteExW.
                unsafe { WaitForSingleObject(self.process, 250) } == WAIT_OBJECT_0;
            if signaled {
                // SAFETY: exited child of our own handle.
                unsafe { GetExitCodeProcess(self.process, &mut rc) };
                return rc;
            }
            if abort_requested() {
                // Short final grace for graceful shutdown.
                let mut late_rc = STILL_ACTIVE;
                unsafe { WaitForSingleObject(self.process, 5000) };
                // SAFETY: read the resulting code regardless of liveness.
                unsafe { GetExitCodeProcess(self.process, &mut late_rc) };
                if late_rc != STILL_ACTIVE {
                    return late_rc;
                }
                return ABANDONED;
            }
        }
    }
}

impl Drop for RunAsChild {
    fn drop(&mut self) {
        if !self.process.is_null() {
            unsafe { CloseHandle(self.process) };
        }
    }
}

/// Port of `relaunchElevatedForWizard`'s ShellExecuteExW("runas") core.
pub fn shellexecute_runas(exe: &str, parameters: &str) -> Option<RunAsChild> {
    let verb = wide("runas");
    let file = wide(exe);
    let params = wide(parameters);
    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_DDEWAIT;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = file.as_ptr();
    sei.lpParameters = params.as_ptr();
    sei.nShow = 1; // SW_SHOWNORMAL
                   // SAFETY: all pointed-to strings outlive the call; on success the child
                   // process handle ownership transfers to RunAsChild.
    let ok = unsafe { ShellExecuteExW(&mut sei) };
    if ok == 0 || sei.hProcess.is_null() {
        None
    } else {
        Some(RunAsChild {
            process: sei.hProcess,
        })
    }
}

// ---------------------------------------------------------------------------
// File primitives
// ---------------------------------------------------------------------------

/// Schedule a path for deletion at next reboot (MoveFileExW, NULL target).
pub fn move_file_ex_delay_until_reboot(path: &str) -> bool {
    let w = wide(path);
    // SAFETY: path string outlives the call.
    unsafe { MoveFileExW(w.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) != 0 }
}

pub fn delete_file_raw(path: &str) -> Result<(), Win32Error> {
    let w = wide(path);
    // SAFETY: path string outlives the call.
    if unsafe { DeleteFileW(w.as_ptr()) } != 0 {
        Ok(())
    } else {
        Err(last_error())
    }
}

pub fn set_file_attributes_normal(path: &str) -> bool {
    let w = wide(path);
    // SAFETY: path string outlives the call.
    unsafe { SetFileAttributesW(w.as_ptr(), FILE_ATTRIBUTE_NORMAL) != 0 }
}

pub fn current_last_error() -> Win32Error {
    last_error()
}

// ---------------------------------------------------------------------------

// Service-control bindings live in the sibling module (same unsafe
// boundary policy). Re-exported here for ergonomic `ffi::` paths.
pub use crate::ffi_services::{
    enumerate_win32_services, open_service_control_manager, StopOutcome,
};

// COM Task Scheduler bindings live in the sibling module (same unsafe
// boundary policy). Re-exported here for ergonomic  paths.
pub use crate::ffi_tasksched::{HrError, TiSession, TiState};
