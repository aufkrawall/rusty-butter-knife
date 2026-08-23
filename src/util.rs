//! String helpers ported 1:1 from the C++ (`toLower`, `wildcardMatchNoCase`,
//! `quoteArg`, `joinCommand`, `parseCsvLine`, minimal JSON readers, ...).
//! Matching operates on lowercased whole strings; case folding is applied to
//! both sides so index arithmetic stays consistent.

/// `towlower`-style per-character lowercase. Rust's `to_lowercase` can expand
/// some chars (ß → ss); matching here only ever compares two fully lowered
/// strings against each other, so expansion is harmless.
pub fn to_lower(s: &str) -> String {
    s.to_lowercase()
}

pub fn contains_no_case(haystack: &str, needle: &str) -> bool {
    to_lower(haystack).contains(&to_lower(needle))
}

pub fn trim(s: &str) -> String {
    s.trim_matches([' ', '\t', '\r', '\n']).to_string()
}

pub fn ends_with_no_case(s: &str, suffix: &str) -> bool {
    let sl = to_lower(s);
    let fl = to_lower(suffix);
    sl.ends_with(&fl)
}

/// Windows argv quoting rules (port of `quoteArg`).
pub fn quote_arg(arg: &str) -> String {
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else if c == '"' {
            for _ in 0..(backslashes * 2 + 1) {
                out.push('\\');
            }
            out.push(c);
            backslashes = 0;
        } else {
            for _ in 0..backslashes {
                out.push('\\');
            }
            backslashes = 0;
            out.push(c);
        }
    }
    for _ in 0..(backslashes * 2) {
        out.push('\\');
    }
    out.push('"');
    out
}

/// Port of `joinCommand`.
pub fn join_command(args: &[String]) -> String {
    let mut out = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let needs_quotes = a.is_empty() || a.contains([' ', '\t', '"', '&', '|', '<', '>', '^']);
        if needs_quotes {
            out.push_str(&quote_arg(a));
        } else {
            out.push_str(a);
        }
    }
    out
}

/// Iterative glob matcher supporting `*` and `?` (port of
/// `wildcardMatchNoCase`).
pub fn wildcard_match_no_case(text_in: &str, pattern_in: &str) -> bool {
    let text: Vec<char> = to_lower(text_in).chars().collect();
    let pattern: Vec<char> = to_lower(pattern_in).chars().collect();
    let (mut t, mut p) = (0usize, 0usize);
    let mut star = usize::MAX;
    let mut matched = 0usize;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            t += 1;
            p += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = p;
            p += 1;
            matched = t;
        } else if star != usize::MAX {
            p = star + 1;
            matched += 1;
            t = matched;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

/// JSON string escaping (port of `jsonEscape`). Non-BMP characters pass
/// through as-is, exactly like the C++ UTF-16-unit loop.
pub fn json_escape(input: &str) -> String {
    let mut out = String::new();
    for c in input.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => {
                if (c as u32) < 0x20 {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                } else {
                    out.push(c);
                }
            }
        }
    }
    out
}

fn skip_ws(json: &[char], mut i: usize) -> usize {
    while i < json.len() && json[i].is_whitespace() {
        i += 1;
    }
    i
}

/// Minimal reader for the status JSON this program itself writes.
/// Good enough for fixed keys; avoids pulling in a JSON dependency.
/// Port of `jsonFindStringField`. (Legacy: kept for status-file parity —
/// the parent now reads LastTaskResult via COM, same as the C++ build.)
#[allow(dead_code)]
pub fn json_find_string_field(json: &str, key: &str) -> String {
    let chars: Vec<char> = json.chars().collect();
    let needle: Vec<char> = format!("\"{key}\"").chars().collect();
    let mut pos = find_sub(&chars, 0, &needle);
    while pos != usize::MAX {
        let colon = match find_ch(&chars, pos + needle.len(), ':') {
            Some(c) => c,
            None => return String::new(),
        };
        let i = skip_ws(&chars, colon + 1);
        if i < chars.len() && chars[i] == '"' {
            let mut out = String::new();
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                if chars[j] == '\\' && j + 1 < chars.len() {
                    j += 1;
                    match chars[j] {
                        '\\' => out.push('\\'),
                        '"' => out.push('"'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{0008}'),
                        'f' => out.push('\u{000C}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            if j + 4 < chars.len() {
                                let hex: String = chars[j + 1..=j + 4].iter().collect();
                                if let Ok(v) = u32::from_str_radix(&hex, 16) {
                                    if let Some(ch) = char::from_u32(v) {
                                        out.push(ch);
                                    }
                                }
                                j += 4;
                            }
                        }
                        other => out.push(other),
                    }
                } else {
                    out.push(chars[j]);
                }
                j += 1;
            }
            return out;
        }
        pos = find_sub(&chars, colon, &needle);
    }
    String::new()
}

/// Port of `jsonFindIntField`.
#[allow(dead_code)]
pub fn json_find_int_field(json: &str, key: &str, default_val: i64) -> i64 {
    let chars: Vec<char> = json.chars().collect();
    let needle: Vec<char> = format!("\"{key}\"").chars().collect();
    let mut pos = find_sub(&chars, 0, &needle);
    while pos != usize::MAX {
        let Some(colon) = find_ch(&chars, pos + needle.len(), ':') else {
            return default_val;
        };
        let i = skip_ws(&chars, colon + 1);
        if i < chars.len() && (chars[i] == '-' || chars[i].is_ascii_digit()) {
            let num: String = chars[i..]
                .iter()
                .take_while(|c| **c == '-' || c.is_ascii_digit())
                .collect();
            return num.parse().unwrap_or(default_val);
        }
        pos = find_sub(&chars, colon, &needle);
    }
    default_val
}

fn find_ch(chars: &[char], from: usize, ch: char) -> Option<usize> {
    chars[from.min(chars.len())..]
        .iter()
        .position(|c| *c == ch)
        .map(|o| o + from)
}

const NO_POS: usize = usize::MAX;

fn find_sub(haystack: &[char], from: usize, needle: &[char]) -> usize {
    if needle.is_empty() {
        return from;
    }
    let start = from.min(haystack.len());
    if haystack.len() >= needle.len() {
        for i in start..=(haystack.len() - needle.len()) {
            if haystack[i..i + needle.len()] == *needle {
                return i;
            }
        }
    }
    NO_POS
}

/// `_wtoi` equivalent: parse a leading (optionally signed) integer, 0 on
/// absence/overflow beyond i32 range is clamped.
pub fn atoi_prefix(s: &str) -> i64 {
    let t = s.trim_start();
    let bytes: Vec<char> = t.chars().collect();
    let mut idx = 0usize;
    let mut neg = false;
    if idx < bytes.len() && (bytes[idx] == '-' || bytes[idx] == '+') {
        neg = bytes[idx] == '-';
        idx += 1;
    }
    let mut value: i128 = 0;
    let mut any = false;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        any = true;
        value = (value * 10 + (bytes[idx] as u8 - b'0') as i128).min(i64::MAX as i128);
        idx += 1;
    }
    if !any {
        return 0;
    }
    let v = if neg { -value } else { value };
    v.clamp(i32::MIN as i64 as i128, i32::MAX as i64 as i128) as i64
}

/// CSV line parser honoring `""` quoting (port of `parseCsvLine`).
pub fn parse_csv_line(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut fields: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            if in_quotes && i + 1 < chars.len() && chars[i + 1] == '"' {
                cur.push('"');
                i += 1;
            } else {
                in_quotes = !in_quotes;
            }
        } else if c == ',' && !in_quotes {
            fields.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
        i += 1;
    }
    fields.push(cur);
    fields
}
