//! Install-media (ISO) discovery for the creation wizard.
//!
//! The wizard used to hand the user a bare text box: type a filename and we
//! run `vm install <name> <iso>`. That's a guess — the file may not be where
//! `vm(8)` looks, and the user has to already know the exact name. Instead we
//! walk the places install media actually lives and let the user *pick* from
//! what's really there:
//!
//!   - `~/installers` (or `$VMCTL_INSTALLERS`) — where the user drops ISOs
//!   - `<datastore>/.iso` — where `vm-bhyve` looks for install media
//!
//! Everything here is pure path/string work over a supplied directory listing
//! so it's testable without touching a filesystem; the thin `scan` wrappers do
//! the real `fs::read_dir` and feed the pure functions.

use std::fs;
use std::path::{Path, PathBuf};

/// A single install-media candidate the user can pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iso {
    /// File name only (what `vm install` wants when the file is in the datastore).
    pub name: String,
    /// Full path on disk (shown in the picker so the user knows which one it is).
    pub path: PathBuf,
    /// Size in bytes, when we could read it.
    pub size: u64,
    /// Which root we found it under (for the picker's grouping label).
    pub source: &'static str,
}

impl Iso {
    /// Human-readable size, matching how the rest of the UI shows disk sizes.
    pub fn size_h(&self) -> String {
        const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
        let mut v = self.size as f64;
        let mut u = 0;
        while v >= 1024.0 && u < UNITS.len() - 1 {
            v /= 1024.0;
            u += 1;
        }
        if u == 0 {
            format!("{}{}", self.size, UNITS[0])
        } else {
            format!("{:.1}{}", v, UNITS[u])
        }
    }
}

/// True when a file name looks like install media.
///
/// `vm(8)` only boots `.iso`, but users drop `.img`/`.bin` raw images too, and
/// a picker that silently hides them is worse than one that shows them. We
/// accept the common optical/raw-image extensions and nothing else.
pub fn is_install_media(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".iso", ".img", ".bin", ".dmg"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Filter + shape raw `(file_name, size)` entries into `Iso` candidates.
///
/// Pure: pass it the names/sizes you saw in a directory. Skips non-media,
/// skips dotfiles (`.DS_Store`, partial downloads like `.foo.iso.part`), and
/// sorts by name so the picker order is stable.
pub fn parse_entries<I, S>(entries: I, source: &'static str) -> Vec<Iso>
where
    I: IntoIterator<Item = (S, u64)>,
    S: Into<String>,
{
    let mut out: Vec<Iso> = entries
        .into_iter()
        .map(|(n, s)| (n.into(), s))
        .filter(|(n, _)| !n.starts_with('.') && is_install_media(n))
        .map(|(name, size)| {
            let path = PathBuf::from(&name);
            Iso { name, path, size, source }
        })
        .collect();
    out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// Scan one directory for install media. Missing/unreadable dirs yield empty —
/// a picker with no entries is a clear signal, not an error.
pub fn scan_dir(dir: &Path, source: &'static str) -> Vec<Iso> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let entries: Vec<(String, u64)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            (name, size)
        })
        .collect();
    // Rewrite paths to the real directory (parse_entries only knows the name).
    parse_entries(entries, source)
        .into_iter()
        .map(|mut iso| {
            iso.path = dir.join(&iso.name);
            iso
        })
        .collect()
}

/// Resolve the *invoking user's* home directory.
///
/// The TUI re-execs under `doas`/`sudo` to get `vm(8)` root, so `$HOME` is
/// the wrong user's home — under doas it's `/root`, and the ISO picker then
/// reports "none found" even though the media sits in `/home/<user>/installers`.
/// `DOAS_USER`/`SUDO_USER` name the real user; fall back to `$HOME` otherwise.
fn invoking_user_home() -> Option<PathBuf> {
    for var in ["DOAS_USER", "SUDO_USER"] {
        if let Ok(user) = std::env::var(var) {
            if user.is_empty() || user == "root" {
                continue;
            }
            if let Some(dir) = home_for_user(&user) {
                return Some(dir);
            }
        }
    }
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Look up a user's home directory the platform way (FreeBSD/Linux: passwd).
///
/// Uses thread-safe `getpwnam_r` — plain `getpwnam` returns a static buffer
/// that the parallel test runner (and std's own temp-dir lookups) race on.
#[cfg(unix)]
fn home_for_user(user: &str) -> Option<PathBuf> {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let name = CString::new(user).ok()?;
    let mut buf = vec![0u8; 4096];
    let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwnam_r(
            name.as_ptr(),
            &mut passwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() || passwd.pw_dir.is_null() {
        return None;
    }
    let dir = unsafe { CStr::from_ptr(passwd.pw_dir) }.to_bytes();
    if dir.is_empty() {
        return None;
    }
    let mut p = PathBuf::new();
    p.push(std::ffi::OsStr::from_bytes(dir));
    Some(p)
}

/// The user's installers directory: `$VMCTL_INSTALLERS`, the invoking user's
/// `~/installers` (even under doas/sudo), then `$HOME/installers`.
pub fn installers_dir() -> PathBuf {
    if let Ok(d) = std::env::var("VMCTL_INSTALLERS") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    let home = invoking_user_home().unwrap_or_else(|| PathBuf::from("/root"));
    home.join("installers")
}

/// The datastore's `.iso` directory (where `vm-bhyve` looks for media).
pub fn datastore_iso_dir(datastore: &Path) -> PathBuf {
    datastore.join(".iso")
}

/// Gather every pickable ISO from both roots, installers first (that's where
/// the user's own media lives), then the datastore. Dedupes by file name so a
/// file present in both doesn't show twice.
pub fn discover(datastore: &Path) -> Vec<Iso> {
    let mut all = scan_dir(&installers_dir(), "installers");
    all.extend(scan_dir(&datastore_iso_dir(datastore), "datastore"));
    all.sort_by(|a, b| {
        // installers before datastore, then name.
        let rank = |s: &str| if s == "installers" { 0 } else { 1 };
        rank(a.source)
            .cmp(&rank(b.source))
            .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
    });
    all.dedup_by(|a, b| a.name == b.name);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_filter_accepts_iso_and_images() {
        assert!(is_install_media("debian-13.7.0-amd64-netinst.iso"));
        assert!(is_install_media("disk.img"));
        assert!(is_install_media("INSTALL.ISO"));
        assert!(!is_install_media("notes.txt"));
        assert!(!is_install_media("checksums.sha256"));
    }

    #[test]
    fn parse_skips_dotfiles_and_non_media() {
        let got = parse_entries(
            [
                ("debian.iso".to_string(), 100u64),
                (".DS_Store".to_string(), 10),
                (".ubuntu.iso.part".to_string(), 5),
                ("readme.txt".to_string(), 3),
                ("alpine.img".to_string(), 200),
            ],
            "installers",
        );
        let names: Vec<&str> = got.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["alpine.img", "debian.iso"]);
    }

    #[test]
    fn parse_sorts_and_dedupes() {
        let got = parse_entries(
            [
                ("b.iso".to_string(), 1u64),
                ("a.iso".to_string(), 1u64),
                ("a.iso".to_string(), 1u64), // exact duplicate collapses
                ("A.iso".to_string(), 1u64), // distinct name (case-sensitive fs)
            ],
            "datastore",
        );
        // Sorted case-insensitively; "a.iso" and "A.iso" tie on the lowercase
        // key so the stable sort keeps their insertion order (a.iso first).
        // The exact duplicate "a.iso" collapsed to one.
        let names: Vec<&str> = got.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["a.iso", "A.iso", "b.iso"]);
    }

    #[test]
    fn size_humanizes() {
        let mk = |size| Iso { name: "x".into(), path: PathBuf::from("x"), size, source: "installers" };
        assert_eq!(mk(512).size_h(), "512B");
        assert_eq!(mk(756 * 1024 * 1024).size_h(), "756.0M");
        assert_eq!(mk(2 * 1024 * 1024 * 1024).size_h(), "2.0G");
    }

    #[test]
    fn missing_dirs_yield_empty() {
        let got = scan_dir(Path::new("/nonexistent/installers"), "installers");
        assert!(got.is_empty());
    }

    #[test]
    fn datastore_dir_is_dot_iso() {
        assert_eq!(
            datastore_iso_dir(Path::new("/zroot/vm")),
            PathBuf::from("/zroot/vm/.iso")
        );
    }

    #[test]
    fn invoking_user_home_ignores_root_and_falls_back_to_env() {
        // Simulate `doas`: DOAS_USER names the real user; HOME is /root.
        // installers_dir must resolve to the invoking user's home, not /root.
        let real_user = std::env::var("USER").unwrap_or_else(|_| "root".into());
        if real_user == "root" {
            return; // can't test user-home resolution when running as root
        }
        std::env::remove_var("VMCTL_INSTALLERS");
        std::env::set_var("DOAS_USER", &real_user);
        std::env::set_var("HOME", "/root");
        let dir = installers_dir();
        // Should point at the invoking user's home, not /root.
        let home = home_for_user(&real_user).expect("passwd lookup for current user");
        assert_eq!(dir, home.join("installers"));
        assert_ne!(dir, PathBuf::from("/root/installers"));
    }
}
