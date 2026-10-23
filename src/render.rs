use crate::Row;
use crate::rules::Regen;

/// A path as safe to print: control characters (newlines would fake extra rows) and bidi
/// overrides become escapes, and invalid UTF-8 bytes become `\xNN`.
pub fn escape_path(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::new();
    for chunk in path.as_os_str().as_bytes().utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
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

/// Binary-unit size: `"512 B"`, `"1.5 KiB"`.
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

pub fn json(rows: &[Row]) -> String {
    serde_json::to_string_pretty(rows).expect("rows always serialize")
}

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
}
