use std::path::PathBuf;
use std::time::SystemTime;

pub mod scan;

/// Everything that depends on the environment, so tests can supply their own.
pub struct Ctx {
    pub home: PathBuf,
    pub tmp: PathBuf,
    pub proc_root: PathBuf,
    pub euid: u32,
    pub now: SystemTime,
}
