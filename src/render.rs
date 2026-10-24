//! Turning candidates and sizes into text that is safe to print.
//!
//! Two things matter here beyond layout:
//!
//! - **File names are untrusted input.** A name can contain a newline (which would draw a fake
//!   row in the table), escape sequences (which would drive the terminal), or bytes that are not
//!   valid text. Every path and every message that may contain a path goes through
//!   [`escape_path`] or [`escape_text`] before it is shown or logged. Output built from these
//!   functions can never contain a raw control character.
//! - **Sizes use binary units** (KiB, MiB, ...), the same as most disk tools, see [`human`].

use crate::Row;
use crate::rules::Regen;

/// Returns `path` in a form that is safe to print and to write to a log.
///
/// What changes:
///
/// - `\n`, `\r` and `\t` become the two characters `\n`, `\r`, `\t`;
/// - other control characters (such as the escape character that starts terminal commands) and
///   the bidirectional text overrides that can reorder text on screen become `\u{hex}`;
/// - bytes that are not valid UTF-8 become `\xNN`;
/// - a backslash becomes two backslashes, so a file literally named `a\nb` never looks the same
///   as one containing a newline.
///
/// Everything else, including non-ASCII letters, is kept. Distinct paths always give distinct
/// results.
pub fn escape_path(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    escape_bytes(path.as_os_str().as_bytes())
}

/// The shared implementation of [`escape_path`] and [`escape_text`], working on raw bytes so that
/// invalid UTF-8 can be represented too.
fn escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                // escaped too, so `\n` in a name never reads like an escaped newline
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                // control characters, and bidi marks/overrides that reorder text on screen
                c if c.is_control()
                    || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') =>
                {
                    out.push_str(&format!("\\u{{{:x}}}", c as u32));
                }
                c => out.push(c),
            }
        }
        for b in chunk.invalid() {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out
}

/// [`escape_path`] for text that may carry a path inside it, such as the message of an I/O
/// error ("permission denied: /tmp/the\nname"). Use it for every message built from the
/// filesystem before showing or logging it.
pub fn escape_text(s: &str) -> String {
    escape_bytes(s.as_bytes())
}

/// Formats a byte count with binary units and at most one decimal: `0 B`, `512 B`, `1.5 KiB`,
/// `5.0 GiB`. Values under 1024 are shown as whole bytes. Units go up to TiB.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

/// Renders `rows` as the table `safesweep scan` prints: a header, one aligned line per row
/// (category, size, regeneration cost, status, escaped path), and a final line with the total
/// of the rows whose status is `ok`. Skipped rows are listed but do not count towards the total.
pub fn table(rows: &[Row]) -> String {
    let regen = |r: Regen| match r {
        Regen::Free => "free",
        Regen::Slow => "slow",
        Regen::Network => "network",
    };
    let cells: Vec<[String; 5]> =
        std::iter::once(["CATEGORY", "SIZE", "REGEN", "STATUS", "PATH"].map(String::from))
            .chain(rows.iter().map(|r| {
                [
                    r.category.to_string(),
                    human(r.bytes),
                    regen(r.regen).to_string(),
                    r.status.clone(),
                    escape_path(&r.path),
                ]
            }))
            .collect();
    let width = |i: usize| cells.iter().map(|c| c[i].len()).max().unwrap_or(0);
    let widths = [width(0), width(1), width(2), width(3)];
    let mut out = String::new();
    for c in &cells {
        for (cell, w) in c.iter().zip(widths) {
            out += &format!("{cell:<w$}  ");
        }
        out += &c[4];
        out.push('\n');
    }
    let total: u64 = rows
        .iter()
        .filter(|r| r.status == "ok")
        .map(|r| r.bytes)
        .sum();
    out += &format!("total reclaimable: {}\n", human(total));
    out
}

/// Renders `rows` as pretty-printed JSON, the output of `safesweep scan --json`.
///
/// Paths are escaped with [`escape_path`], so the output is always valid JSON even for file
/// names that are not valid UTF-8.
///
/// # Panics
///
/// Never in practice: a [`Row`] contains only strings and numbers, which always serialize.
pub fn json(rows: &[Row]) -> String {
    serde_json::to_string_pretty(rows).expect("rows always serialize")
}

/// Unit tests for escaping, sizes and the table and JSON output.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{RULES, Regen};
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    fn row(path: PathBuf, bytes: u64, status: &str) -> Row {
        Row {
            category: "tmp",
            path,
            bytes,
            regen: Regen::Free,
            status: status.into(),
            rule: RULES.iter().find(|r| r.id == "tmp").unwrap(),
        }
    }

    #[test]
    fn human_units() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1536), "1.5 KiB");
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[test]
    fn table_total_ok_only() {
        let rows = [
            row("/a".into(), 2048, "ok"),
            row("/b".into(), 4096, "skipped: symlink"),
        ];
        let out = table(&rows);
        assert!(out.starts_with("CATEGORY"));
        assert!(
            out.trim_end().ends_with("total reclaimable: 2.0 KiB"),
            "{out}"
        );
    }

    #[test]
    fn json_non_utf8_path() {
        let rows = [row(PathBuf::from(OsStr::from_bytes(b"bad\xff")), 1, "ok")];
        let v: serde_json::Value = serde_json::from_str(&json(&rows)).unwrap();
        // invalid bytes are escaped (not replaced), so distinct names stay distinct
        assert_eq!(v[0]["path"], "bad\\xff");
        assert!(table(&rows).contains("bad"));
    }

    #[test]
    fn escape_path_hides_newlines_bidi_and_bad_bytes() {
        let p = |b: &[u8]| escape_path(std::path::Path::new(OsStr::from_bytes(b)));
        assert_eq!(p(b"/plain/ok.txt"), "/plain/ok.txt");
        assert_eq!(p(b"x\n[ ] fake"), "x\\n[ ] fake");
        assert_eq!(p(b"a\r\tb"), "a\\r\\tb");
        assert_eq!(p(b"bad\xff"), "bad\\xff");
        assert_eq!(p("a\u{202e}b".as_bytes()), "a\\u{202e}b");
        assert_eq!(p("a\x1b[31mb".as_bytes()), "a\\u{1b}[31mb");
        assert_eq!(p("naïve/日本".as_bytes()), "naïve/日本");
    }

    #[test]
    fn table_cannot_be_spoofed_by_newline_in_name() {
        let rows = [row(PathBuf::from("/t/x\n[ ] 9.9 GiB fake"), 1, "ok")];
        assert_eq!(table(&rows).lines().count(), 3); // header, one row, total
    }

    #[test]
    fn escape_text_and_backslash() {
        assert_eq!(
            escape_text("IO error on /t/x\nok 9.9 GiB\x1b[2J"),
            "IO error on /t/x\\nok 9.9 GiB\\u{1b}[2J"
        );
        // a literal backslash-n in a name must not print the same as a real newline
        let p = |b: &[u8]| escape_path(std::path::Path::new(OsStr::from_bytes(b)));
        assert_ne!(p(b"a\\nb"), p(b"a\nb"));
        assert_eq!(p(b"a\\b"), "a\\\\b");
    }
}
