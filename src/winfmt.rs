//! Win32 formatting/encoding helpers: timestamps, error messages, and the
//! OEM-codepage decoding of external tool output. 100% safe (via `ffi`).

use crate::ffi;
use crate::util::trim;

/// `%04u%02u%02u-%02u%02u%02u` local-time stamp (port of `nowStamp`).
pub fn now_stamp() -> String {
    let (y, mo, d, h, mi, s) = ffi::local_time_parts();
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// Log-line timestamp body: `%04u-%02u-%02u %02u:%02u:%02u`.
pub fn log_time_stamp() -> String {
    let (y, mo, d, h, mi, s) = ffi::local_time_parts();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Port of `formatWinError`: `"0x%08X <message>"`.
pub fn format_win_error(err: u32) -> String {
    let mut out = format!("0x{err:08X}");
    if let Some(msg) = ffi::format_message(err) {
        let t = trim(&msg);
        if !t.is_empty() {
            out.push(' ');
            out.push_str(&t);
        }
    }
    out
}

/// Console tools write their stdout pipes in the OEM code page, not ANSI and
/// not UTF-8. Decode strictly as UTF-8 first (some tools do emit UTF-8),
/// then via the OEM code page. Port of `decodeProcessOutput`.
pub fn decode_process_output(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if std::str::from_utf8(bytes).is_ok() {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let oem_cp = ffi::get_oem_code_page();
    ffi::mb_to_wide(oem_cp, 0, bytes)
}
