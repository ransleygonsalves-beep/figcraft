//! A file's version history on the server, for files published by an account: versions people
//! save by name, and checkpoints the server keeps by itself while the file is edited (every
//! [`CHECKPOINT_EVERY`]) and when everyone has left. Each version says who saved it and who
//! changed the file since the one before, from the names of the people whose changes the session
//! accepted (an account's name is its own; it can't be made up).
//!
//! ```text
//! <data>/files/<id>/versions/index.json  the versions, oldest first (see [`Kept`])
//! <data>/files/<id>/versions/<v>.bin     a version's design (see [`disk::encode_changes`])
//! ```

use figcraft_collab::message::{AccountUser, VersionInfo};
use figcraft_collab::{Change, Journal, Server};
use serde::{Deserialize, Serialize};

use super::{META, Store, disk, now, write_json};

/// How often a file being edited gets a checkpoint (seconds).
pub const CHECKPOINT_EVERY: u64 = 30 * 60;
/// Checkpoints kept per file; the oldest makes room.
pub const MAX_CHECKPOINTS: usize = 20;
/// Versions kept per file; the oldest checkpoint makes room, named ones are never dropped.
pub const MAX_VERSIONS: usize = 100;
/// The longest version name and description kept.
const MAX_NAME: usize = 200;
const MAX_DESCRIPTION: usize = 2000;
/// Most editors listed per version.
const MAX_EDITORS: usize = 50;
/// What a checkpoint is called.
const CHECKPOINT: &str = "Autosave";

const VERSIONS: &str = "versions";
const INDEX: &str = "index.json";

/// One version as `index.json` keeps it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Kept {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    time: u64,
    #[serde(default)]
    auto: bool,
    #[serde(default)]
    by: String,
    #[serde(default)]
    editors: Vec<String>,
}

impl Kept {
    fn info(&self) -> VersionInfo {
        VersionInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            time: i64::try_from(self.time).unwrap_or(i64::MAX),
            auto: self.auto,
            by: self.by.clone(),
            editors: self.editors.clone(),
        }
    }
}

fn clip(s: &str, max: usize) -> String {
    s.trim().chars().filter(|c| !c.is_control()).take(max).collect()
}

/// Whether `id` could be a version's id (made by [`Store::keep_version`]: lowercase hex).
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Store {
    fn versions_dir(&self, file: &str) -> std::path::PathBuf {
        self.dir(file).join(VERSIONS)
    }

    /// `file`'s versions, oldest first (none when it has no history yet, or it can't be read).
    fn kept(&self, file: &str) -> Vec<Kept> {
        let path = self.versions_dir(file).join(INDEX);
        match disk::read_capped(&path, 4 << 20) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                log::warn!("file {file}: its version history can't be read ({e})");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        }
    }

    /// Whether `file` keeps a version history: it was published by an account.
    fn has_history(&self, file: &str) -> bool {
        self.files.get(file).is_some_and(|e| e.meta.account_owner.is_some())
    }

    /// `file`'s history, newest first.
    pub fn versions(&self, file: &str) -> Vec<VersionInfo> {
        self.kept(file).iter().rev().map(Kept::info).collect()
    }

    /// Keep `snapshot` as a version of `file` (`by`: who saved it; empty for a checkpoint), with
    /// who edited since the last one; they start over.
    fn keep_version(&mut self, file: &str, snapshot: &[Change], name: &str, description: &str, by: &str) -> Result<VersionInfo, String> {
        let auto = by.is_empty();
        let dir = self.versions_dir(file);
        let mut kept = self.kept(file);
        let checkpoints = kept.iter().filter(|v| v.auto).count();
        let mut dropped = Vec::new();
        if ((auto && checkpoints >= MAX_CHECKPOINTS) || kept.len() >= MAX_VERSIONS)
            && let Some(oldest) = kept.iter().position(|v| v.auto)
        {
            dropped.push(kept.remove(oldest));
        }
        if kept.len() >= MAX_VERSIONS {
            return Err(format!("this file's version history is full ({MAX_VERSIONS} saved versions): delete old ones first"));
        }
        let mut t = now();
        let mut id = format!("{t:x}");
        // Two in the same second get the next free id.
        while kept.iter().any(|v| v.id == id) {
            t += 1;
            id = format!("{t:x}");
        }
        let editors = self.files.get(file).map(|e| e.meta.editors.clone()).unwrap_or_default();
        let v =
            Kept { id, name: clip(name, MAX_NAME), description: clip(description, MAX_DESCRIPTION), time: now(), auto, by: by.to_string(), editors };
        std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't keep the version: {e}"))?;
        let bytes = disk::encode_changes(snapshot)?;
        disk::write_atomic(&dir.join(format!("{}.bin", v.id)), &bytes).map_err(|e| format!("couldn't keep the version: {e}"))?;
        kept.push(v.clone());
        if let Err(e) = write_json(&dir.join(INDEX), &kept) {
            let _ = std::fs::remove_file(dir.join(format!("{}.bin", v.id)));
            return Err(format!("couldn't keep the version: {e}"));
        }
        for old in dropped {
            let _ = std::fs::remove_file(dir.join(format!("{}.bin", old.id)));
        }
        // Who edited is told by this version now.
        let meta_path = self.dir(file).join(META);
        if let Some(e) = self.files.get_mut(file) {
            e.meta.editors.clear();
            e.meta.checkpoint = v.time;
            if let Err(err) = write_json(&meta_path, &e.meta) {
                log::warn!("file {file}: couldn't note its new version ({err})");
            }
        }
        log::info!("event=version file={file} id={} auto={auto}", v.id);
        Ok(v.info())
    }

    /// Note who made the changes the session just accepted, and keep a checkpoint when the file
    /// has been edited for [`CHECKPOINT_EVERY`] since its last version.
    pub(super) fn note_editors(&mut self, file: &str, server: &Server, journal: &[Journal]) {
        if !self.has_history(file) {
            return;
        }
        let names: Vec<String> = journal
            .iter()
            .filter_map(|j| match j {
                Journal::Accepted(a) => server.peers().find(|p| p.client_id == a.author).map(|p| p.name.clone()),
                _ => None,
            })
            .collect();
        if names.is_empty() {
            return;
        }
        let meta_path = self.dir(file).join(META);
        let Some(e) = self.files.get_mut(file) else { return };
        let mut new = false;
        for n in names {
            if !e.meta.editors.contains(&n) && e.meta.editors.len() < MAX_EDITORS {
                e.meta.editors.push(n);
                new = true;
            }
        }
        if e.meta.checkpoint == 0 {
            // Its history starts with its first edit.
            e.meta.checkpoint = now();
            new = true;
        }
        if new && let Err(err) = write_json(&meta_path, &e.meta) {
            log::warn!("file {file}: couldn't note who edited it ({err})");
        }
        if now().saturating_sub(e.meta.checkpoint) >= CHECKPOINT_EVERY {
            self.checkpoint(file, server);
        }
    }

    /// Keep a checkpoint of `file` as `server` has it, when someone changed it since its last
    /// version.
    pub(super) fn checkpoint(&mut self, file: &str, server: &Server) {
        if !self.has_history(file) || self.files.get(file).is_none_or(|e| e.meta.editors.is_empty()) {
            return;
        }
        let kept = server.replica().snapshot().map_err(|e| e.to_string()).and_then(|snap| self.keep_version(file, &snap, CHECKPOINT, "", ""));
        if let Err(e) = kept {
            log::warn!("file {file}: no checkpoint was kept: {e}");
        }
    }

    /// Save `file` as it is now as a version named `name`, as `user` asks (they may edit it);
    /// `live`: its design when it is open.
    pub fn save_version(&mut self, user: &AccountUser, file: &str, name: &str, description: &str, live: Option<Vec<Change>>) -> Result<(), String> {
        if self.access_of(user, file) != Some(figcraft_collab::message::Access::Edit) {
            return Err("only someone who may edit the file can save a version of it".into());
        }
        if clip(name, MAX_NAME).is_empty() {
            return Err("give the version a name".into());
        }
        let snapshot = match live {
            Some(s) => s,
            None => self.closed_design(file)?,
        };
        self.keep_version(file, &snapshot, name, description, &user.name).map(|_| ())
    }

    /// The design of `file` when nobody is in it: from the session kept loaded after everyone
    /// left, else read from disk (and kept loaded a while, as if someone had just left).
    fn closed_design(&mut self, file: &str) -> Result<Vec<Change>, String> {
        let server = self.load(file)?;
        let snapshot = server.replica().snapshot().map_err(|e| e.to_string());
        if let Some(log) = self.files.get_mut(file).and_then(|e| e.log.take()).filter(|l| l.pending.is_empty()) {
            self.park(file, server, log);
        }
        snapshot
    }

    /// One version of `file` with its design, as `user` asks (they may open the file).
    pub fn version(&self, user: &AccountUser, file: &str, id: &str) -> Result<(VersionInfo, Vec<Change>), String> {
        if self.access_of(user, file).is_none() {
            return Err("this file isn't shared with your account".into());
        }
        let v =
            self.kept(file).into_iter().find(|v| v.id == id).filter(|_| valid_id(id)).ok_or("there's no such version (it may have been deleted)")?;
        let path = self.versions_dir(file).join(format!("{id}.bin"));
        let bytes = disk::read_capped(&path, self.snapshot_cap())?;
        Ok((v.info(), disk::decode_changes(&bytes)?))
    }

    /// `file`'s history, as `user` asks (they may open it).
    pub fn versions_for(&self, user: &AccountUser, file: &str) -> Result<Vec<VersionInfo>, String> {
        if self.access_of(user, file).is_none() {
            return Err("this file isn't shared with your account".into());
        }
        Ok(self.versions(file))
    }

    /// Rename a version (`Some`) or delete it (`None`), as `user` asks (they manage the file).
    pub fn change_version(&mut self, user: &AccountUser, file: &str, id: &str, rename: Option<(&str, &str)>) -> Result<(), String> {
        self.account_manages(user, file)?;
        let dir = self.versions_dir(file);
        let mut kept = self.kept(file);
        let i = kept.iter().position(|v| v.id == id).ok_or("there's no such version (it may have been deleted)")?;
        match rename {
            Some((name, description)) => {
                let name = clip(name, MAX_NAME);
                if name.is_empty() {
                    return Err("give the version a name".into());
                }
                let v = &mut kept[i];
                v.name = name;
                v.description = clip(description, MAX_DESCRIPTION);
                // A named checkpoint is a saved version: never dropped to make room.
                v.auto = false;
                write_json(&dir.join(INDEX), &kept).map_err(|e| format!("couldn't rename the version: {e}"))
            }
            None => {
                let gone = kept.remove(i);
                write_json(&dir.join(INDEX), &kept).map_err(|e| format!("couldn't delete the version: {e}"))?;
                let _ = std::fs::remove_file(dir.join(format!("{}.bin", gone.id)));
                Ok(())
            }
        }
    }
}
