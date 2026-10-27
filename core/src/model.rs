//! Typed guest state and size helpers shared by every front-end.
//!
//! `vm list` / `vm info` report state as free text (`Running (1234)`,
//! `Locked (nas01)`, `running (90212)`, `Bootloader (77)`). Front-ends should
//! never string-match that themselves — they go through [`VmState`].

/// A guest's lifecycle state as reported by vm-bhyve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmState {
    Running(Option<u32>),
    /// Guest is in its bhyveload/grub stage (vm-bhyve reports `Bootloader (pid)`).
    Bootloader(Option<u32>),
    Stopped,
    Suspended,
    /// `run.lock` present but no local process: another host owns it.
    Locked(String),
    Other(String),
}

fn paren(s: &str) -> Option<&str> {
    let a = s.find('(')?;
    let b = s.rfind(')')?;
    (b > a).then(|| s[a + 1..b].trim())
}

impl VmState {
    /// Parse a state string from `vm list` (STATE column) or `vm info` (`state:`).
    pub fn parse(raw: &str) -> Self {
        let s = raw.trim();
        let lower = s.to_ascii_lowercase();
        let pid = || paren(s).and_then(|p| p.parse().ok());
        if lower.starts_with("running") {
            VmState::Running(pid())
        } else if lower.starts_with("bootloader") {
            VmState::Bootloader(pid())
        } else if lower.starts_with("stopped") {
            VmState::Stopped
        } else if lower.starts_with("suspended") {
            VmState::Suspended
        } else if lower.starts_with("locked") {
            VmState::Locked(paren(s).unwrap_or("unknown host").to_string())
        } else {
            VmState::Other(s.to_string())
        }
    }

    /// Coarse kind used for colours / filters: Running, Stopped, Suspended, Locked, Other.
    /// Bootloader counts as Running — a process exists and holds resources.
    pub fn kind(&self) -> &'static str {
        match self {
            VmState::Running(_) | VmState::Bootloader(_) => "Running",
            VmState::Stopped => "Stopped",
            VmState::Suspended => "Suspended",
            VmState::Locked(_) => "Locked",
            VmState::Other(_) => "Other",
        }
    }

    /// Long label, e.g. `Running (4182)` or `Locked (nas01)`.
    pub fn label(&self) -> String {
        match self {
            VmState::Running(Some(p)) => format!("Running ({p})"),
            VmState::Running(None) => "Running".into(),
            VmState::Bootloader(Some(p)) => format!("Bootloader ({p})"),
            VmState::Bootloader(None) => "Bootloader".into(),
            VmState::Stopped => "Stopped".into(),
            VmState::Suspended => "Suspended".into(),
            VmState::Locked(h) => format!("Locked ({h})"),
            VmState::Other(s) => s.clone(),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, VmState::Running(_) | VmState::Bootloader(_))
    }

    /// Holds host resources (counts toward the allocation bars).
    pub fn holds_resources(&self) -> bool {
        self.is_running() || matches!(self, VmState::Suspended)
    }
}

/// Parse a vm-bhyve memory/size token (`256M`, `4096M`, `8G`, `1T`, `512K`,
/// bare number = bytes per bhyve convention is rare; we treat bare as MiB)
/// into MiB.
pub fn parse_mib(s: &str) -> Option<u64> {
    let t = s.trim().trim_matches('"');
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if digits.is_empty() {
        return None;
    }
    let n: f64 = digits.parse().ok()?;
    let unit = t[digits.len()..].trim().to_ascii_uppercase();
    let mib = match unit.trim_end_matches('B') {
        "" | "M" => n,
        "K" => n / 1024.0,
        "G" => n * 1024.0,
        "T" => n * 1024.0 * 1024.0,
        _ => return None,
    };
    Some(mib.round() as u64)
}

/// Format MiB the way the UI shows memory: `4G`, `1.5G`, `256M`.
pub fn fmt_mib(mib: u64) -> String {
    if mib >= 1024 {
        let g = mib as f64 / 1024.0;
        if (g - g.round()).abs() < 0.05 {
            format!("{}G", g.round() as u64)
        } else {
            format!("{g:.1}G")
        }
    } else {
        format!("{mib}M")
    }
}

/// Human byte size (`3.1G`, `412M`, `0B`).
pub fn fmt_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b}B")
    } else if v >= 100.0 {
        format!("{v:.0}{}", U[i])
    } else {
        format!("{v:.1}{}", U[i])
    }
}

/// Valid vm-bhyve guest name: letters, digits, `-`, `_`, `.` not leading.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && !n.starts_with('.')
        && !n.starts_with('-')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `base-01`, `base-02`, … first one not in `taken`.
pub fn next_name(base: &str, taken: &[String]) -> String {
    (1..1000)
        .map(|n| format!("{base}-{n:02}"))
        .find(|c| !taken.iter().any(|t| t == c))
        .unwrap_or_else(|| format!("{base}-new"))
}

/// Local wall-clock formatting of a unix timestamp (no chrono dependency).
pub fn fmt_local(ts: i64, with_date: bool) -> String {
    let t = ts as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r writes into our stack-owned tm and is thread-safe.
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if !ok {
        return ts.to_string();
    }
    if with_date {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min
        )
    } else {
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

/// `MMDD-HHMM` stamp for auto-named snapshots.
pub fn stamp(ts: i64) -> String {
    let full = fmt_local(ts, true); // YYYY-MM-DD HH:MM
    if full.len() >= 16 {
        format!("{}{}-{}{}", &full[5..7], &full[8..10], &full[11..13], &full[14..16])
    } else {
        ts.to_string()
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_states() {
        assert_eq!(VmState::parse("Running (4182)"), VmState::Running(Some(4182)));
        assert_eq!(VmState::parse("running (90212)"), VmState::Running(Some(90212)));
        assert_eq!(VmState::parse("Stopped"), VmState::Stopped);
        assert_eq!(VmState::parse("stopped"), VmState::Stopped);
        assert_eq!(VmState::parse("Suspended"), VmState::Suspended);
        assert_eq!(VmState::parse("Locked (nas01)"), VmState::Locked("nas01".into()));
        assert_eq!(VmState::parse("Bootloader (77)").kind(), "Running");
        assert_eq!(VmState::parse("weird").kind(), "Other");
    }

    #[test]
    fn labels_round_trip() {
        for s in ["Running (12)", "Stopped", "Suspended", "Locked (h)"] {
            assert_eq!(VmState::parse(s).label(), s);
        }
    }

    #[test]
    fn mem_parsing() {
        assert_eq!(parse_mib("256M"), Some(256));
        assert_eq!(parse_mib("4096M"), Some(4096));
        assert_eq!(parse_mib("8G"), Some(8192));
        assert_eq!(parse_mib("\"4G\""), Some(4096));
        assert_eq!(parse_mib("1T"), Some(1024 * 1024));
        assert_eq!(parse_mib("x"), None);
        assert_eq!(fmt_mib(4096), "4G");
        assert_eq!(fmt_mib(1536), "1.5G");
        assert_eq!(fmt_mib(256), "256M");
    }

    #[test]
    fn names() {
        assert!(valid_name("ubuntu-01"));
        assert!(!valid_name("bad name"));
        assert!(!valid_name("-x"));
        let taken = vec!["ubuntu-01".to_string()];
        assert_eq!(next_name("ubuntu", &taken), "ubuntu-02");
    }

    #[test]
    fn bytes() {
        assert_eq!(fmt_bytes(0), "0B");
        assert_eq!(fmt_bytes(1024 * 1024 * 3), "3.0M");
    }
}
