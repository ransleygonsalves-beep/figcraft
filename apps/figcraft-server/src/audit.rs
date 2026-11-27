//! The audit log (`<data>/audit.log`, owner-only on Unix): one JSON line per sign-in (made or
//! refused), account made, password reset, agent token made or ended, change of who may open a
//! file, account change, file deleted, and version deleted or restored, with the account and the
//! address it came from. Passwords, codes and tokens are never written. Past [`MAX_BYTES`] it
//! moves to `audit.log.1` (the one before is dropped) and starts again.

use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The file's name in the data directory.
pub const AUDIT: &str = "audit.log";
/// How large the log grows before it starts over (the last one is kept beside it).
const MAX_BYTES: u64 = 16 << 20;

/// Where audit events go.
pub struct Audit {
    path: PathBuf,
}

impl Audit {
    pub fn new(root: &Path) -> Audit {
        Audit { path: root.join(AUDIT) }
    }

    /// Note `event` by account `user` (`None`: not signed in) from `ip`, with `detail` (never a
    /// secret). A write that fails is reported in the server's log; the request goes on.
    pub fn note(&self, event: &str, user: Option<u64>, ip: Option<IpAddr>, detail: Value) {
        let mut line = json!({"time": crate::store::now(), "event": event, "account": user, "ip": ip.map(|i| i.to_string())});
        if let (Value::Object(l), Value::Object(d)) = (&mut line, detail) {
            l.extend(d);
        }
        if let Err(e) = self.append(&line) {
            log::warn!("couldn't write the audit log: {e}");
        }
    }

    /// The latest `limit` events, newest first (from the log, then the one before it when the log
    /// has fewer). Lines that aren't events are skipped.
    pub fn recent(&self, limit: usize) -> Vec<Value> {
        let mut out = Vec::new();
        for path in [self.path.clone(), self.path.with_extension("log.1")] {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            out.extend(text.lines().rev().filter_map(|l| serde_json::from_str::<Value>(l).ok()).take(limit - out.len()));
            if out.len() >= limit {
                break;
            }
        }
        out
    }

    fn append(&self, line: &Value) -> std::io::Result<()> {
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > MAX_BYTES) {
            let old = self.path.with_extension("log.1");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&self.path, &old)?;
        }
        let mut o = std::fs::OpenOptions::new();
        o.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        let mut f = o.open(&self.path)?;
        let mut bytes = serde_json::to_vec(line).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        f.write_all(&bytes)
    }
}
