//! Shared activity log — every command either front-end runs, with its real
//! exit status and stderr.
//!
//! Persisted as one tab-separated line per entry at
//! `$XDG_STATE_HOME/vmctl/activity.tsv` (default `~/.local/state/vmctl/`), so
//! the desktop and the TUI see each other's history. Tabs/newlines inside
//! fields are escaped. The file is append-only; readers take the last N lines.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::backend::ExecResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub ts: i64,
    /// Which front-end ran it (`desktop`, `tui`).
    pub source: String,
    pub cmd: String,
    pub code: i32,
    pub stderr: String,
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl Entry {
    pub fn from_result(source: &str, r: &ExecResult) -> Self {
        Self { ts: crate::model::now(), source: source.into(), cmd: r.cmd.clone(), code: r.code, stderr: r.stderr.clone() }
    }

    pub fn to_line(&self) -> String {
        format!("{}\t{}\t{}\t{}\t{}", self.ts, esc(&self.source), self.code, esc(&self.cmd), esc(&self.stderr))
    }

    pub fn parse(line: &str) -> Option<Self> {
        let mut p = line.splitn(5, '\t');
        Some(Self {
            ts: p.next()?.parse().ok()?,
            source: unesc(p.next()?),
            code: p.next()?.parse().ok()?,
            cmd: unesc(p.next()?),
            stderr: unesc(p.next().unwrap_or("")),
        })
    }
}

/// Resolve the log path for the *invoking* user (works under sudo/doas too).
pub fn default_path() -> PathBuf {
    if let Ok(p) = std::env::var("VMCTL_ACTIVITY") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(x) = std::env::var("XDG_STATE_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("vmctl/activity.tsv");
        }
    }
    crate::iso::installers_dir()
        .parent()
        .map(|h| h.join(".local/state/vmctl/activity.tsv"))
        .unwrap_or_else(|| PathBuf::from("/tmp/vmctl-activity.tsv"))
}

pub struct ActivityLog {
    path: Option<PathBuf>,
    /// Newest first.
    pub entries: Vec<Entry>,
    cap: usize,
}

impl ActivityLog {
    /// In-memory only (demo mode / tests).
    pub fn memory() -> Self {
        Self { path: None, entries: Vec::new(), cap: 500 }
    }

    pub fn open(path: PathBuf) -> Self {
        let mut log = Self { path: Some(path), entries: Vec::new(), cap: 500 };
        log.reload();
        log
    }

    /// Re-read the file so entries written by the other front-end show up.
    pub fn reload(&mut self) {
        let Some(p) = &self.path else { return };
        if let Ok(raw) = fs::read_to_string(p) {
            let mut v: Vec<Entry> = raw.lines().filter_map(Entry::parse).collect();
            let start = v.len().saturating_sub(self.cap);
            v.drain(..start);
            v.reverse();
            self.entries = v;
        }
    }

    /// Record an entry. A write failure is returned, never swallowed silently;
    /// the entry is still kept in memory.
    pub fn push(&mut self, e: Entry) -> std::io::Result<()> {
        self.entries.insert(0, e.clone());
        self.entries.truncate(self.cap);
        let Some(p) = &self.path else { return Ok(()) };
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut f = OpenOptions::new().create(true).append(true).open(p)?;
        writeln!(f, "{}", e.to_line())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_round_trip_with_escapes() {
        let e = Entry { ts: 5, source: "desktop".into(), cmd: "vm start a".into(), code: 1, stderr: "line1\nline\t2 \\ok".into() };
        assert_eq!(Entry::parse(&e.to_line()), Some(e));
    }

    #[test]
    fn file_persists_and_reloads_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/activity.tsv");
        let mut a = ActivityLog::open(p.clone());
        a.push(Entry { ts: 1, source: "tui".into(), cmd: "vm list".into(), code: 0, stderr: String::new() }).unwrap();
        a.push(Entry { ts: 2, source: "desktop".into(), cmd: "vm start x".into(), code: 1, stderr: "boom".into() }).unwrap();
        let b = ActivityLog::open(p);
        assert_eq!(b.entries.len(), 2);
        assert_eq!(b.entries[0].cmd, "vm start x");
        assert_eq!(b.entries[1].source, "tui");
    }
}
