//! Guest-template discovery for vm-bhyve.
//!
//! `vm-bhyve` ships with **no named templates** — a template only exists once
//! someone drops a `<name>.conf` into `<datastore>/.templates/`. Handing the
//! user a name that isn't there makes `vm create` fail with
//! `unable to find template ...` after they press Enter, which reads like a bug
//! in the wizard even though it's the host being honest.
//!
//! So we read the real set from disk and drive both front-ends off it, instead
//! of hardcoding a list that may not match the machine we're running on.
//!
//! Everything here is pure string/path work so it's testable without a live
//! hypervisor; the front-ends supply the datastore path they discovered.

use std::fs;
use std::path::Path;

/// The OS names we ship sensible defaults for (loader + ISO suggestions).
/// These are *suggestions*, never authoritative — the disk is authoritative.
pub const KNOWN: [&str; 6] = ["ubuntu", "freebsd", "debian", "windows", "openbsd", "alpine"];

/// Parse template ids out of a directory listing (filenames, `.conf` stripped).
///
/// Pure, so tests don't touch a filesystem: pass it the names you saw.
pub fn parse_template_names<I, S>(entries: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out: Vec<String> = entries
        .into_iter()
        .map(|e| e.as_ref().trim().to_string())
        .filter(|e| !e.is_empty() && !e.starts_with('.'))
        .filter_map(|e| e.strip_suffix(".conf").map(str::to_string))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Discover the templates actually installed under `templates_dir`
/// (i.e. `<datastore>/.templates`).
///
/// Returns a sorted, deduped list. If the directory is missing or unreadable we
/// fall back to [`KNOWN`] so the app still works on a fresh host — the fallback
/// is flagged by the caller when it renders.
pub fn discover_in(templates_dir: &Path) -> Vec<String> {
    match fs::read_dir(templates_dir) {
        Ok(rd) => {
            let names: Vec<String> = rd.filter_map(|e| e.ok()).filter_map(|e| {
                e.file_name().to_str().map(str::to_string)
            }).collect();
            let found = parse_template_names(names);
            if found.is_empty() { KNOWN.iter().map(|s| s.to_string()).collect() } else { found }
        }
        Err(_) => KNOWN.iter().map(|s| s.to_string()).collect(),
    }
}

/// Guess the install ISO to show for a template id.
///
/// Purely a display hint — the real install medium is whatever the user boots.
/// Unknown ids get a conventional filename so the field is never blank.
pub fn guess_iso(id: &str) -> String {
    match id {
        "ubuntu" => "ubuntu-24.04-live-server.iso".into(),
        "freebsd" => "FreeBSD-14.3-RELEASE-amd64-disc1.iso".into(),
        "debian" => "debian-13-netinst.iso".into(),
        "windows" => "Win11_24H2.iso".into(),
        "openbsd" => "OpenBSD-7.7-install-amd64.iso".into(),
        "alpine" => "alpine-virt-3.21.0-x86_64.iso".into(),
        other => format!("{other}-install.iso"),
    }
}

/// Suggest a starting guest name for a template id (`ubuntu` → `ubuntu-01`).
pub fn suggest_name(id: &str) -> String {
    format!("{id}-01")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_conf_and_sorts() {
        let got = parse_template_names(["freebsd.conf", "ubuntu.conf", "alpine.conf"]);
        assert_eq!(got, vec!["alpine", "freebsd", "ubuntu"]);
    }

    #[test]
    fn ignores_dirs_and_hidden() {
        let got = parse_template_names([".hidden.conf", "README", "debian.conf", "sub.dir"]);
        assert_eq!(got, vec!["debian"]);
    }

    #[test]
    fn dedupes() {
        let got = parse_template_names(["a.conf", "a.conf"]);
        assert_eq!(got, vec!["a"]);
    }

    #[test]
    fn missing_dir_falls_back_to_known() {
        let got = discover_in(Path::new("/nonexistent/.templates"));
        assert_eq!(got.len(), KNOWN.len());
        assert!(got.contains(&"freebsd".to_string()));
    }

    #[test]
    fn iso_guess_covers_known_and_generates_for_unknown() {
        assert_eq!(guess_iso("ubuntu"), "ubuntu-24.04-live-server.iso");
        assert!(guess_iso("plan9").ends_with("-install.iso"));
    }

    #[test]
    fn suggests_name_from_template() {
        assert_eq!(suggest_name("freebsd"), "freebsd-01");
    }
}
