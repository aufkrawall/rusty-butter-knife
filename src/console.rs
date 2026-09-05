//! Console output (WriteConsoleW for full Unicode on real consoles, UTF-8
//! bytes when redirected), colors, and the stdin line reader.
//! 100% safe — raw console calls live in `ffi`.

use std::io::Write;

use crate::ffi;

const FOREGROUND_BLUE: u16 = 0x0001;
const FOREGROUND_GREEN: u16 = 0x0002;
const FOREGROUND_RED: u16 = 0x0004;
const FOREGROUND_INTENSITY: u16 = 0x0008;
pub const COLOR_WHITE: u16 = FOREGROUND_RED | FOREGROUND_GREEN | FOREGROUND_BLUE;
pub const COLOR_RED: u16 = FOREGROUND_RED | FOREGROUND_INTENSITY;
pub const COLOR_YELLOW: u16 = FOREGROUND_RED | FOREGROUND_GREEN | FOREGROUND_INTENSITY;
pub const COLOR_CYAN: u16 = FOREGROUND_BLUE | FOREGROUND_GREEN | FOREGROUND_INTENSITY;

fn is_console(h: isize) -> bool {
    h != 0 && ffi::is_console(h)
}

fn encode_crlf_utf16(text: &str) -> Vec<u16> {
    let mut text16: Vec<u16> = Vec::with_capacity(text.len() + 1);
    let mut prev = 0u16;
    for unit in text.encode_utf16() {
        if unit == b'\n' as u16 && prev != b'\r' as u16 {
            text16.push(b'\r' as u16);
        }
        text16.push(unit);
        prev = unit;
    }
    text16
}

/// Write to stdout via WriteConsoleW when attached to a console (Unicode-safe
/// on any console code page), UTF-8 bytes otherwise.
pub fn out(text: &str) {
    let h = ffi::stdout_handle();
    if is_console(h) {
        let text16 = encode_crlf_utf16(text);
        let _ = ffi::write_console_w(h, &text16);
    } else {
        let mut lock = std::io::stdout().lock();
        let _ = lock.write_all(text.as_bytes());
    }
}

/// Same for stderr.
pub fn err_out(text: &str) {
    let h = ffi::stderr_handle();
    if is_console(h) {
        let text16 = encode_crlf_utf16(text);
        let _ = ffi::write_console_w(h, &text16);
    } else {
        let mut lock = std::io::stderr().lock();
        let _ = lock.write_all(text.as_bytes());
    }
}

/// Set console foreground color; no-op when `enabled` is false or redirected.
pub fn set_color(color: u16, enabled: bool) {
    if !enabled {
        return;
    }
    let h = ffi::stdout_handle();
    if is_console(h) {
        ffi::set_console_text_attribute(h, color);
    }
}

/// Read one line from stdin; returns None on EOF/closed input — mirrors
/// `!std::getline(std::wcin, ...)`.
pub fn read_line() -> Option<String> {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => {
            while line.ends_with('\n') || line.ends_with('\r') {
                line.pop();
            }
            Some(line)
        }
        Err(_) => None,
    }
}

/// Flush helper retained for parity with explicit `std::wcout.flush()` calls.
pub fn flush() {
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_crlf_utf16_normalizes_line_endings() {
        // Plain text without newlines
        let plain = encode_crlf_utf16("hello");
        assert_eq!(String::from_utf16(&plain).unwrap(), "hello");

        // Bare LF becomes CRLF
        let lf = encode_crlf_utf16("hello\nworld\n");
        assert_eq!(String::from_utf16(&lf).unwrap(), "hello\r\nworld\r\n");

        // Pre-existing CRLF is not doubled to CRCRLF
        let crlf = encode_crlf_utf16("hello\r\nworld\r\n");
        assert_eq!(String::from_utf16(&crlf).unwrap(), "hello\r\nworld\r\n");
    }
}

