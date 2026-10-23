use crate::Row;
use crate::rules::Regen;

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
                    r.path.display().to_string(),
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
            rule: &RULES[6],
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
        assert!(json(&rows).contains("bad"));
        assert!(table(&rows).contains("bad"));
    }
}
