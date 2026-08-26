//! Child-process creation with BOUNDED stdout/stderr capture. Split out of
//! `ffi.rs` along its section boundary; same policy applies: this is part of
//! the FFI boundary module family, so `unsafe` is permitted here only.
//!
//! Hard-bounds contract (regression guard for audit finding HANG-01):
//! - Output is ONLY ever drained through PeekNamedPipe-gated reads of data
//!   known to be buffered (`drain_available`). No blocking ReadFile-until-EOF
//!   anywhere, because an inherited pipe write handle kept alive by a
//!   descendant would otherwise block forever after the direct child exits.
//! - On timeout the direct child is terminated and only awaited within a
//!   short grace window; the pipe is closed regardless of descendants.
//! - Only the intended pipe handle is inherited (PROC_THREAD_ATTRIBUTE_
//!   HANDLE_LIST), not every inheritable handle in the process.

#![allow(unsafe_code)]

use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_NO_WINDOW, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use crate::ffi::{format_message, last_error, wide};

/// Outcome carrier for [`capture_process`].
pub struct CaptureResult {
    pub exit_code: u32,
    pub output: String,
    pub started: bool,
    /// True when `timeout_ms` elapsed and the direct child was terminated.
    pub timed_out: bool,
    /// True when the abort callback fired during the wait (Ctrl+C family).
    pub aborted: bool,
}

const TERMINATE_TIMEOUT_CODE: u32 = 0xFFFF_0001;
const TERMINATE_ABORT_CODE: u32 = 0xFFFF_0002;
/// Short bounded grace for observing termination; the pipe is closed
/// regardless of descendants afterwards.
const KILL_GRACE_MS: u32 = 1000;

fn win_err_text(err: u32) -> String {
    format_message(err).unwrap_or_else(|| format!("error {err}"))
}

/// Drain everything currently buffered in the pipe WITHOUT ever waiting for
/// more data or EOF. Safe against descendants holding the write handle:
/// every ReadFile is preceded by a PeekNamedPipe reporting data available,
/// so it can only return buffered bytes at most once before re-checking.
fn drain_available(pipe: HANDLE, bytes: &mut Vec<u8>, chunk: &mut [u8]) {
    loop {
        let mut avail = 0u32;
        let peek_ok =
            // SAFETY: valid owned read-handle; out-param written here.
            unsafe {
                PeekNamedPipe(
                    pipe,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut avail,
                    std::ptr::null_mut(),
                )
            } != 0;
        if !peek_ok || avail == 0 {
            return; // broken pipe (EOF/no writer) or nothing buffered: stop.
        }
        let to_read = avail.min(chunk.len() as u32);
        let mut got = 0u32;
        // SAFETY: bounded read of exactly to_read bytes into chunk; cannot
        // block because avail > 0 was just reported for this buffer state.
        let rd_ok = unsafe {
            ReadFile(
                pipe,
                chunk.as_mut_ptr().cast(),
                to_read,
                &mut got,
                std::ptr::null_mut(),
            )
        } != 0;
        if !rd_ok || got == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..got as usize]);
    }
}

/// Owns everything the attribute-list pointers reference. Fields drop after
/// Drop::drop runs, so DeleteProcThreadAttributeList executes while both the
/// list storage and the referenced HANDLE array are still alive.
struct ProcThreadAttrs {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    /// Attribute-list storage the `list` pointer references.
    _buf: Vec<u8>,
    /// lpValue target for PROC_THREAD_ATTRIBUTE_HANDLE_LIST. MSDN requires
    /// the value buffer to outlive the attribute-list deletion.
    _value: Box<[HANDLE; 1]>,
}

impl Drop for ProcThreadAttrs {
    fn drop(&mut self) {
        if !self.list.is_null() {
            // SAFETY: list was created successfully iff non-null here.
            unsafe { DeleteProcThreadAttributeList(self.list) };
        }
    }
}

/// Build a one-entry HANDLE_LIST attribute (the pipe write end only), so no
/// unrelated inheritable handle leaks into the child.
fn make_handle_list_attr(write_pipe: HANDLE) -> Result<ProcThreadAttrs, String> {
    let mut size = 0usize;
    // SAFETY: first call with NULL only reports the required size.
    // MSDN: this probe call "fails" with ERROR_INSUFFICIENT_BUFFER by design
    // while filling `size`; only a zero size means real failure.
    let _probe_ok =
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) };
    if size == 0 {
        return Err(format!(
            "InitializeProcThreadAttributeList(size query) failed: {}",
            win_err_text(last_error())
        ));
    }
    let mut buf = vec![0u8; size];
    let list = buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    // SAFETY: buf stays alive in ProcThreadAttrs until after
    // DeleteProcThreadAttributeList runs (see Drop impl).
    let ok = unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) };
    if ok == 0 {
        return Err(format!(
            "InitializeProcThreadAttributeList failed: {}",
            win_err_text(last_error())
        ));
    }
    let value: Box<[HANDLE; 1]> = Box::new([write_pipe]);
    // SAFETY: value/size match the HANDLE_LIST contract (array of HANDLEs);
    // the box lives until after the list is deleted and after CreateProcessW.
    let ok = unsafe {
        UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            value.as_ptr() as *mut _,
            std::mem::size_of::<HANDLE>(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(format!(
            "UpdateProcThreadAttribute(HANDLE_LIST) failed: {}",
            win_err_text(last_error())
        ));
    }
    Ok(ProcThreadAttrs {
        list,
        _buf: buf,
        _value: value,
    })
}

/// Capture stdout+stderr of `command_line` with hard time bounds:
/// 50 ms wait slices, drain-only-available reads, terminate + short grace on
/// timeout, abort-aware (see module docs). `decode` converts raw pipe bytes.
pub fn capture_process(
    command_line: &str,
    timeout_ms: u32,
    decode: impl Fn(&[u8]) -> String,
    abort_requested: impl Fn() -> bool,
) -> CaptureResult {
    let mut result = CaptureResult {
        exit_code: 0xFFFF_FFFF,
        output: String::new(),
        started: false,
        timed_out: false,
        aborted: false,
    };

    let sa = windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut read_pipe: HANDLE = std::ptr::null_mut();
    let mut write_pipe: HANDLE = std::ptr::null_mut();
    // SAFETY: both pipe ends come back owned; write end closed right after
    // CreateProcessW, read end on every return path below.
    if unsafe { CreatePipe(&mut read_pipe, &mut write_pipe, &sa, 0) } == 0 {
        result.output = format!("CreatePipe failed: {}", win_err_text(last_error()));
        return result;
    }
    // Fail closed: a read end accidentally inherited would keep the child's
    // stdout open forever even after all children exited.
    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    // SAFETY: read end must stay ours; the error is checked per contract.
    if unsafe {
        windows_sys::Win32::Foundation::SetHandleInformation(read_pipe, HANDLE_FLAG_INHERIT, 0)
    } == 0
    {
        result.output = "SetHandleInformation(read pipe non-inheritable) failed.".to_string();
        unsafe { CloseHandle(write_pipe) };
        unsafe { CloseHandle(read_pipe) };
        return result;
    }

    let created = make_handle_list_attr(write_pipe);
    let attrs = match created {
        Ok(a) => a,
        Err(msg) => {
            result.output = msg;
            unsafe { CloseHandle(write_pipe) };
            unsafe { CloseHandle(read_pipe) };
            return result;
        }
    };

    let mut siex: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    siex.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    siex.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    siex.StartupInfo.hStdOutput = write_pipe;
    siex.StartupInfo.hStdError = write_pipe;
    siex.lpAttributeList = attrs.list;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    // CreateProcessW may write into the command line buffer.
    let mut cmd_w = wide(command_line);
    let ok = unsafe {
        CreateProcessW(
            std::ptr::null(),
            cmd_w.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            std::ptr::null(),
            std::ptr::null(),
            &siex.StartupInfo,
            &mut pi,
        )
    };
    drop(attrs);
    unsafe { CloseHandle(write_pipe) };
    if ok == 0 {
        result.output = format!(
            "CreateProcess failed: {} Command={command_line}",
            win_err_text(last_error())
        );
        unsafe { CloseHandle(read_pipe) };
        return result;
    }

    result.started = true;
    let mut bytes: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let start = Instant::now();
    const WAIT_OBJECT_0: u32 = 0;

    loop {
        drain_available(read_pipe, &mut bytes, &mut chunk);

        if abort_requested() {
            // Abandon the child promptly instead of riding out the timeout.
            result.aborted = true;
            // SAFETY: terminate our own direct child; then bounded grace.
            unsafe { TerminateProcess(pi.hProcess, TERMINATE_ABORT_CODE) };
            unsafe { WaitForSingleObject(pi.hProcess, KILL_GRACE_MS) };
            drain_available(read_pipe, &mut bytes, &mut chunk);
            break;
        }

        if unsafe { WaitForSingleObject(pi.hProcess, 50) } == WAIT_OBJECT_0 {
            // Normal exit: drain what is ALREADY buffered and stop — never a
            // blocking read-to-EOF (descendants may still hold the handle).
            drain_available(read_pipe, &mut bytes, &mut chunk);
            break;
        }
        if start.elapsed() >= Duration::from_millis(u64::from(timeout_ms)) {
            result.timed_out = true;
            // SAFETY: terminate our own direct child; then bounded grace.
            unsafe { TerminateProcess(pi.hProcess, TERMINATE_TIMEOUT_CODE) };
            unsafe { WaitForSingleObject(pi.hProcess, KILL_GRACE_MS) };
            drain_available(read_pipe, &mut bytes, &mut chunk);
            result.output.push_str("Timed out. ");
            break;
        }
    }

    unsafe {
        // SAFETY: exit-code query + cleanup on our owned handles.
        GetExitCodeProcess(pi.hProcess, &mut result.exit_code);
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        CloseHandle(read_pipe);
    }
    result.output.push_str(&decode(&bytes));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_utf8(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// A genuinely hung child must be terminated by the hard timeout, and the
    /// whole call must return within the timeout plus a small grace — never
    /// block past it.
    #[test]
    fn hung_child_is_terminated_by_timeout() {
        let start = Instant::now();
        let res = capture_process("ping -n 60 127.0.0.1", 1200, decode_utf8, || false);
        let elapsed = start.elapsed();
        assert!(res.started, "child should start: {}", res.output);
        assert!(res.timed_out, "hard timeout must fire");
        assert!(
            elapsed < Duration::from_secs(6),
            "bounded wait, got {elapsed:?}"
        );
        // Direct child was ping.exe itself; terminate-on-timeout prevents a
        // lingering process.
    }

    /// High-volume stdout must be captured completely through the bounded
    /// drain (no truncation from PeekNamedPipe-only reads).
    #[test]
    fn high_volume_output_captured_completely() {
        let res = capture_process(
            "cmd /c (for /l %i in (1,1,4000) do @echo line-%i)",
            30_000,
            decode_utf8,
            || false,
        );
        assert!(res.started, "started: {}", res.output);
        assert!(!res.timed_out);
        assert_eq!(res.output.lines().count(), 4000, "all lines drained");
        assert!(res.output.contains("line-1\n") || res.output.contains("line-1\r"));
        assert!(res.output.contains("line-4000"));
    }

    /// Regression for HANG-01: after the direct child exits, a descendant can
    /// still hold an inherited stdout write handle. The old implementation
    /// performed a blocking ReadFile-until-EOF and waited out that
    /// descendant; the new one returns as soon as the direct child exits,
    /// draining only already-buffered bytes. The started grandchild ping is
    /// harmless and self-exits after ~9 s.
    #[test]
    fn descendant_holding_stdout_does_not_block() {
        // cmd exits immediately after handing the pipe to the inner `start`
        // child; its stdout write end stays open via the grandchild.
        let command_line =
            "cmd /c start \"gpd-desc\" /min cmd /c \"ping -n 10 127.0.0.1 >nul\" & exit";
        let start = Instant::now();
        let res = capture_process(command_line, 60_000, decode_utf8, || false);
        let elapsed = start.elapsed();
        assert!(res.started);
        assert!(!res.timed_out);
        assert!(
            elapsed < Duration::from_secs(5),
            "must return right after the direct child exits (took {elapsed:?}); \
             blocking here means the pipe was read to EOF again"
        );
    }
}
