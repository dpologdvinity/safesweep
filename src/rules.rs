//! The rule table: which locations `safesweep` knows how to clean, and the conditions for each.
//!
//! A [`Rule`] is plain data. It names a root directory, says how candidates are picked under it
//! (a [`Matcher`]), how long they must have been untouched, and what it costs to get the data
//! back. All behavior lives elsewhere ([`crate::scan`] finds, [`crate::guard`] decides,
//! [`crate::clean`] removes), so adding a location means adding one entry to [`RULES`] and
//! nothing else.
//!
//! The tool only ever considers paths a rule names. There is no "find big old files" mode: if
//! no rule covers a path, `safesweep` will not touch it.

use crate::Ctx;
use crate::scan::Matcher;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// What it costs to get the deleted data back, shown to the user next to each candidate.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Regen {
    /// Recreated automatically and instantly (`__pycache__`, temp files).
    Free,
    /// Recreated automatically, but only by redoing slow work, such as a rebuild.
    Slow,
    /// Recreated by downloading again (package manager caches).
    Network,
}

/// Where a rule's search starts.
pub enum Root {
    /// A path relative to the user's home directory (`$HOME`). The empty string is the home
    /// directory itself.
    Home(&'static str),
    /// The system temporary directory (`$TMPDIR`, or `/tmp`).
    Tmp,
}

/// One cleanable location. See the module docs.
pub struct Rule {
    /// Short name shown in the output and accepted by `--category` (for example `pip`).
    pub id: &'static str,
    /// Where the search starts. Candidates must stay inside this directory.
    pub root: Root,
    /// How candidates are chosen under the root.
    pub matcher: Matcher,
    /// How many days nothing inside a candidate may have changed before it can be deleted.
    pub min_age_days: u64,
    /// What it costs to get the data back.
    pub regen: Regen,
    /// If true, the rule is left out of the default set and only runs when named with
    /// `--category`. Used for rules that walk the whole home directory (slow) or delete things
    /// that take minutes to rebuild.
    pub opt_in: bool,
}

impl Rule {
    /// The directory that must be unused by any running process before `path` may be deleted.
    ///
    /// Normally that is the candidate itself. For build directories it is the whole project (the
    /// parent): a server running from the project has its working directory there, not inside
    /// `node_modules`, yet deleting its dependencies would break it.
    pub fn busy_scope<'a>(&self, path: &'a Path) -> &'a Path {
        match self.matcher {
            Matcher::IdleArtifacts => path.parent().unwrap_or(path),
            _ => path,
        }
    }

    /// The absolute root directory of this rule for the environment in `ctx`.
    pub fn root_path(&self, ctx: &Ctx) -> PathBuf {
        match self.root {
            Root::Home(rel) => ctx.home.join(rel),
            Root::Tmp => ctx.tmp.clone(),
        }
    }
}

/// Every rule `safesweep` knows, in the order they are scanned.
///
/// The ages are deliberately short for caches that are cheap to refill (one day for package
/// manager downloads) and longer where deletion is more disruptive (seven days for other
/// application caches and temp files, thirty for build directories of idle projects).
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
    // Opt-in: this walks the whole home directory, which took 9+ minutes on a machine with
    // millions of files. Zero days: stale bytecode is harmless, and Python rewrites it.
    Rule {
        id: "pycache",
        root: Root::Home(""),
        matcher: Matcher::NamedDirs("__pycache__"),
        min_age_days: 0,
        regen: Regen::Free,
        opt_in: true,
    },
    // Opt-in: walks the home directory (slow) and deletes things that take minutes to rebuild.
    // Only idle projects are touched; see `Matcher::IdleArtifacts`.
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

/// Picks the rules named by `ids`, or the default set if `ids` is empty.
///
/// The default set is every rule that is not [`Rule::opt_in`]. Naming an opt-in rule selects it
/// explicitly, and naming rules never adds the rest of the defaults.
///
/// # Errors
///
/// Returns a message listing the valid names if any id is unknown.
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
/// Unit tests for rule selection.
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
