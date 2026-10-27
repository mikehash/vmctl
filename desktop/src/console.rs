//! Embedded serial console (F-108, desktop flavour).
//!
//! vm-bhyve wires each guest's com1 to an nmdm(4) pair: bhyve holds the `A`
//! end, we open the `B` end. The device is root-only, so we open it through a
//! tiny `sh` helper (under `sudo -n` when we're not root) that copies the
//! device to stdout and our stdin to the device. Closing stdin ends the helper.
//!
//! This is a *line* console, not a terminal emulator: ANSI escape sequences
//! are stripped and cursor movement is ignored. That's plenty for login
//! prompts, boot logs and shells; for full-screen programs use "Open in
//! terminal", which runs the real `vm console` in a terminal emulator.

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};

const HELPER: &str = r#"dev="$1"
exec 3<>"$dev" || exit 1
stty raw -echo <&3 2>/dev/null
cat <&3 &
reader=$!
cat >&3
kill "$reader" 2>/dev/null
"#;

pub enum Session {
    Live { child: Child, stdin: Option<ChildStdin>, rx: Receiver<Vec<u8>> },
    Demo { name: String, stage: u8, line: String },
}

impl Session {
    pub fn open(dev: &str, sudo: bool) -> std::io::Result<Self> {
        let mut c = if sudo {
            let mut c = Command::new("sudo");
            c.args(["-n", "/bin/sh", "-c", HELPER, "vmctl-console", dev]);
            c
        } else {
            let mut c = Command::new("/bin/sh");
            c.args(["-c", HELPER, "vmctl-console", dev]);
            c
        };
        let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let mut out = child.stdout.take().expect("piped stdout");
        let mut err = child.stderr.take().expect("piped stderr");
        let (tx, rx) = channel();
        let tx2 = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = err.read_to_end(&mut s);
            if !s.is_empty() {
                let mut v = b"\n[console helper] ".to_vec();
                v.extend(s);
                let _ = tx2.send(v);
            }
        });
        let stdin = child.stdin.take();
        Ok(Session::Live { child, stdin, rx })
    }

    pub fn demo(name: &str) -> Self {
        Session::Demo { name: name.into(), stage: 0, line: String::new() }
    }

    pub fn greeting(&self) -> String {
        match self {
            Session::Live { .. } => String::new(),
            Session::Demo { name, .. } => format!(
                "[  OK  ] Started ssh.service - OpenBSD Secure Shell server.\n[  OK  ] Reached target multi-user.target.\n\n(demo console — nothing here talks to a real guest)\n\n{name} login: "
            ),
        }
    }

    /// Bytes from the guest since last poll (None = session ended).
    pub fn poll(&mut self) -> Option<Vec<u8>> {
        match self {
            Session::Live { child, rx, .. } => {
                let mut out = Vec::new();
                while let Ok(b) = rx.try_recv() {
                    out.extend(b);
                }
                if out.is_empty() {
                    if let Ok(Some(_)) = child.try_wait() {
                        return None;
                    }
                }
                Some(out)
            }
            Session::Demo { .. } => Some(Vec::new()),
        }
    }

    pub fn send(&mut self, bytes: &[u8]) -> Vec<u8> {
        match self {
            Session::Live { stdin, .. } => {
                if let Some(s) = stdin.as_mut() {
                    let _ = s.write_all(bytes).and_then(|_| s.flush());
                }
                Vec::new()
            }
            Session::Demo { name, stage, line } => {
                // A toy getty: echoes, then fakes login → password → shell.
                let mut echo = Vec::new();
                for &b in bytes {
                    match b {
                        b'\r' | b'\n' => {
                            echo.extend(b"\n");
                            let prompt = match *stage {
                                0 => "Password: ".to_string(),
                                // The password line is swallowed, never run or echoed.
                                1 => format!("mike@{name}:~$ "),
                                _ => {
                                    let out = match line.trim() {
                                        "" => String::new(),
                                        "uname -a" => "FreeBSD demo 14.3-RELEASE amd64\n".into(),
                                        cmd => format!("sh: {cmd}: not found (demo)\n"),
                                    };
                                    format!("{out}mike@{name}:~$ ")
                                }
                            };
                            *stage = (*stage + 1).min(2);
                            line.clear();
                            echo.extend(prompt.as_bytes());
                        }
                        0x08 | 0x7f => {
                            if line.pop().is_some() && *stage != 1 {
                                echo.push(0x08);
                            }
                        }
                        c if c >= 0x20 => {
                            line.push(c as char);
                            if *stage != 1 {
                                echo.push(c);
                            }
                        }
                        _ => {}
                    }
                }
                echo
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Session::Live { child, stdin, .. } = self {
            drop(stdin.take()); // helper's `cat >&3` sees EOF and exits
            std::thread::sleep(std::time::Duration::from_millis(50));
            if let Ok(None) = child.try_wait() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

/// Map a Slint key event to the bytes a serial terminal would send.
pub fn key_bytes(text: &str, ctrl: bool) -> Vec<u8> {
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.clone().next()) else {
        return text.as_bytes().to_vec();
    };
    if ctrl && c.is_ascii_alphabetic() {
        return vec![(c.to_ascii_lowercase() as u8) - b'a' + 1];
    }
    match c {
        '\n' | '\r' => b"\r".to_vec(),
        '\u{8}' => vec![0x7f],
        '\u{7f}' => b"\x1b[3~".to_vec(),
        '\t' => b"\t".to_vec(),
        '\u{1b}' => vec![0x1b],
        '\u{F700}' => b"\x1b[A".to_vec(),
        '\u{F701}' => b"\x1b[B".to_vec(),
        '\u{F702}' => b"\x1b[D".to_vec(),
        '\u{F703}' => b"\x1b[C".to_vec(),
        '\u{F729}' => b"\x1b[H".to_vec(),
        '\u{F72B}' => b"\x1b[F".to_vec(),
        '\u{F72C}' => b"\x1b[5~".to_vec(),
        '\u{F72D}' => b"\x1b[6~".to_vec(),
        c if ('\u{F700}'..='\u{F8FF}').contains(&c) => Vec::new(), // other function keys
        c => c.to_string().into_bytes(),
    }
}

/// A scrollback buffer that understands just enough of a terminal stream.
#[derive(Default)]
pub struct Screen {
    lines: Vec<String>,
    esc: u8, // 0 none, 1 saw ESC, 2 in CSI, 3 in OSC
    utf8: Vec<u8>,
}

impl Screen {
    const MAX_LINES: usize = 600;

    pub fn feed(&mut self, bytes: &[u8]) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        for &b in bytes {
            match self.esc {
                1 => {
                    self.esc = match b {
                        b'[' => 2,
                        b']' => 3,
                        _ => 0,
                    };
                    continue;
                }
                2 => {
                    if (0x40..=0x7e).contains(&b) {
                        self.esc = 0;
                    }
                    continue;
                }
                3 => {
                    if b == 0x07 || b == 0x1b {
                        self.esc = if b == 0x1b { 1 } else { 0 };
                    }
                    continue;
                }
                _ => {}
            }
            match b {
                0x1b => self.esc = 1,
                b'\n' => {
                    self.lines.push(String::new());
                    if self.lines.len() > Self::MAX_LINES {
                        self.lines.remove(0);
                    }
                }
                b'\r' | 0x07 | 0x00 => {}
                0x08 => {
                    self.lines.last_mut().map(|l| l.pop());
                }
                b'\t' => self.lines.last_mut().unwrap().push_str("    "),
                b if b < 0x20 => {}
                b if b < 0x80 => self.lines.last_mut().unwrap().push(b as char),
                b => {
                    self.utf8.push(b);
                    if let Ok(s) = std::str::from_utf8(&self.utf8) {
                        self.lines.last_mut().unwrap().push_str(s);
                        self.utf8.clear();
                    } else if self.utf8.len() >= 4 {
                        self.utf8.clear();
                    }
                }
            }
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_strips_ansi_and_handles_backspace() {
        let mut s = Screen::default();
        s.feed(b"\x1b[1;32mOK\x1b[0m\r\nlogin: abx\x08c");
        assert_eq!(s.text(), "OK\nlogin: abc");
    }

    #[test]
    fn keys_map_to_terminal_bytes() {
        assert_eq!(key_bytes("\n", false), b"\r");
        assert_eq!(key_bytes("c", true), vec![3]);
        assert_eq!(key_bytes("\u{F700}", false), b"\x1b[A");
        assert_eq!(key_bytes("é", false), "é".as_bytes());
    }

    #[test]
    fn demo_session_fakes_login() {
        let mut d = Session::demo("box");
        let e = d.send(b"root\r");
        assert!(String::from_utf8_lossy(&e).contains("Password: "));
        let e = d.send(b"pw\r");
        assert!(!String::from_utf8_lossy(&e).contains("pw"));
        assert!(String::from_utf8_lossy(&e).contains("mike@box"));
    }
}
