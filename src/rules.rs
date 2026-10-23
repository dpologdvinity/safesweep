use crate::Ctx;
use crate::scan::Matcher;
use serde::Serialize;
use std::path::{Path, PathBuf};

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
    /// Left out of the default set; runs only when named with `--category`.
    pub opt_in: bool,
}

impl Rule {
    /// What must be unused by any process before `path` may go.
    pub fn busy_scope<'a>(&self, path: &'a Path) -> &'a Path {
        match self.matcher {
            Matcher::IdleArtifacts => path.parent().unwrap_or(path),
            _ => path,
        }
    }

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
        opt_in: false,
    },
    Rule {
        id: "pip",
        root: Root::Home(".cache/pip"),
        matcher: Matcher::Whole,
        min_age_days: 1,
        regen: Regen::Network,
        opt_in: false,
    },
    Rule {
        id: "uv",
        root: Root::Home(".cache/uv"),
        matcher: Matcher::Whole,
        min_age_days: 1,
        regen: Regen::Network,
        opt_in: false,
    },
    Rule {
        id: "npm",
        root: Root::Home(".npm/_cacache"),
        matcher: Matcher::Whole,
        min_age_days: 1,
        regen: Regen::Network,
        opt_in: false,
    },
    Rule {
        id: "cargo",
        root: Root::Home(".cargo/registry/cache"),
        matcher: Matcher::Whole,
        min_age_days: 1,
        regen: Regen::Network,
        opt_in: false,
    },
    // Opt-in: walking all of $HOME took 9+ minutes on a dev box with millions of files.
    Rule {
        id: "pycache",
        root: Root::Home(""),
        matcher: Matcher::NamedDirs("__pycache__"),
        min_age_days: 0,
        regen: Regen::Free,
        opt_in: true,
    },
    // Opt-in: walks $HOME and deletes things that take minutes to rebuild.
    Rule {
        id: "artifacts",
        root: Root::Home(""),
        matcher: Matcher::IdleArtifacts,
        min_age_days: 30,
        regen: Regen::Slow,
        opt_in: true,
    },
    Rule {
        id: "tmp",
        root: Root::Tmp,
        matcher: Matcher::Children { exclude: &[] },
        min_age_days: 7,
        regen: Regen::Free,
        opt_in: false,
    },
];

/// Rules with the given ids, or every non-opt-in rule if `ids` is empty.
pub fn select(ids: &[String]) -> Result<Vec<&'static Rule>, String> {
    if ids.is_empty() {
        return Ok(RULES.iter().filter(|r| !r.opt_in).collect());
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
    fn select_default_skips_opt_in() {
        let ids: Vec<_> = select(&[]).unwrap().iter().map(|r| r.id).collect();
        assert_eq!(ids, ["xdg-cache", "pip", "uv", "npm", "cargo", "tmp"]);
    }

    #[test]
    fn select_opt_in_by_name() {
        let ids: Vec<_> = select(&["pycache".into()])
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, ["pycache"]);
    }

    #[test]
    fn select_unknown_errs() {
        assert_eq!(
            select(&["nope".into()]).err(),
            Some(
                "unknown category 'nope'; valid: xdg-cache, pip, uv, npm, cargo, pycache, artifacts, tmp"
                    .into()
            )
        );
    }
}
