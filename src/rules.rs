use crate::Ctx;
use crate::scan::Matcher;
use serde::Serialize;
use std::path::PathBuf;

/// What it costs to get the deleted data back.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Regen {
    Free,
    Slow,
    Network,
}

pub enum Root {
    /// Relative to `$HOME`.
    Home(&'static str),
    /// The system temp dir (`$TMPDIR` or `/tmp`).
    Tmp,
}

pub struct Rule {
    pub id: &'static str,
    pub root: Root,
    pub matcher: Matcher,
    pub min_age_days: u64,
    pub regen: Regen,
}

impl Rule {
    pub fn root_path(&self, ctx: &Ctx) -> PathBuf {
        match self.root {
            Root::Home(rel) => ctx.home.join(rel),
            Root::Tmp => ctx.tmp.clone(),
        }
    }
}

pub static RULES: &[Rule] = &[
    Rule {
        id: "xdg-cache",
        root: Root::Home(".cache"),
        matcher: Matcher::Children {
            exclude: &["pip", "uv"],
        },
        min_age_days: 7,
        regen: Regen::Network,
    },
    Rule {
        id: "pip",
        root: Root::Home(".cache/pip"),
        matcher: Matcher::Whole,
        min_age_days: 1,
        regen: Regen::Network,
    },
    Rule {
        id: "uv",
        root: Root::Home(".cache/uv"),
        matcher: Matcher::Whole,
        min_age_days: 1,
        regen: Regen::Network,
    },
    Rule {
        id: "npm",
        root: Root::Home(".npm/_cacache"),
        matcher: Matcher::Whole,
        min_age_days: 1,
        regen: Regen::Network,
    },
    Rule {
        id: "cargo",
        root: Root::Home(".cargo/registry/cache"),
        matcher: Matcher::Whole,
        min_age_days: 1,
        regen: Regen::Network,
    },
    // shortcut: walks all of $HOME single-threaded per rule; switch to jwalk if scans of large homes are slow.
    Rule {
        id: "pycache",
        root: Root::Home(""),
        matcher: Matcher::NamedDirs("__pycache__"),
        min_age_days: 0,
        regen: Regen::Free,
    },
    Rule {
        id: "tmp",
        root: Root::Tmp,
        matcher: Matcher::Children { exclude: &[] },
        min_age_days: 7,
        regen: Regen::Free,
    },
];

/// Rules with the given ids, or all rules if `ids` is empty.
pub fn select(ids: &[String]) -> Result<Vec<&'static Rule>, String> {
    if ids.is_empty() {
        return Ok(RULES.iter().collect());
    }
    ids.iter()
        .map(|id| {
            RULES.iter().find(|r| r.id == id).ok_or_else(|| {
                let valid: Vec<_> = RULES.iter().map(|r| r.id).collect();
                format!("unknown category '{id}'; valid: {}", valid.join(", "))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_all_when_empty() {
        assert_eq!(select(&[]).unwrap().len(), 7);
    }

    #[test]
    fn select_unknown_errs() {
        assert_eq!(
            select(&["nope".into()]).err(),
            Some(
                "unknown category 'nope'; valid: xdg-cache, pip, uv, npm, cargo, pycache, tmp"
                    .into()
            )
        );
    }
}
