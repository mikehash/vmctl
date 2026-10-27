//! Parser for `vm info` output.
//!
//! Shape (vm-bhyve 1.7.5):
//! ```text
//! ------------------------
//! Virtual Machine: qtumvalidator10
//! ------------------------
//!   state: running (90212)
//!   cpu: 2
//!   memory: 4096M
//!
//!   console-ports
//!     com1: /dev/nmdm-qtumvalidator10.1B
//!
//!   network-interface
//!     number: 0
//!     virtual-switch: public
//! ```
//! Top-level keys are indented 2 spaces, section members 4. `vm info` with no
//! name prints every guest back to back, so we split on `Virtual Machine:`.

use std::collections::BTreeMap;

use crate::model::VmState;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Nic {
    pub number: String,
    pub emulation: String,
    pub switch: String,
    pub mac: String,
    pub device: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Disk {
    pub number: String,
    pub device_type: String,
    pub emulation: String,
    pub path: String,
    /// `bytes-size` human part, e.g. `20.000G`.
    pub size: String,
    pub used: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmInfo {
    pub name: String,
    pub state: VmState,
    pub fields: BTreeMap<String, String>,
    pub consoles: Vec<(String, String)>,
    pub nics: Vec<Nic>,
    pub disks: Vec<Disk>,
}

impl VmInfo {
    pub fn get(&self, k: &str) -> &str {
        self.fields.get(k).map(|s| s.as_str()).unwrap_or("")
    }
}

/// `21474836480 (20.000G)` → `20.000G`; plain values pass through.
fn human_part(v: &str) -> String {
    match (v.find('('), v.rfind(')')) {
        (Some(a), Some(b)) if b > a => v[a + 1..b].to_string(),
        _ => v.to_string(),
    }
}

pub fn parse_info(raw: &str) -> Vec<VmInfo> {
    let mut out: Vec<VmInfo> = Vec::new();
    let mut section = String::new();
    let mut map: BTreeMap<String, String> = BTreeMap::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("Virtual Machine:") {
            if let Some(prev) = out.last_mut() {
                flush(prev, &section, &mut map);
            }
            out.push(VmInfo {
                name: name.trim().to_string(),
                state: VmState::Other(String::new()),
                fields: BTreeMap::new(),
                consoles: Vec::new(),
                nics: Vec::new(),
                disks: Vec::new(),
            });
            section.clear();
            continue;
        }
        let Some(cur) = out.last_mut() else { continue };
        if trimmed.is_empty() || trimmed.starts_with("---") {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        match trimmed.split_once(':') {
            None => {
                // section header ("network-interface", "virtual-disk", ...)
                flush(cur, &section, &mut map);
                section = trimmed.to_string();
            }
            Some((k, v)) => {
                let (k, v) = (k.trim().to_string(), v.trim().to_string());
                if indent <= 2 {
                    flush(cur, &section, &mut map);
                    section.clear();
                    if k == "state" {
                        cur.state = VmState::parse(&v);
                    }
                    cur.fields.insert(k, v);
                } else if section == "console-ports" {
                    cur.consoles.push((k, v));
                } else {
                    // A repeated `number:` starts a new item in the same section.
                    if k == "number" && map.contains_key("number") {
                        flush(cur, &section, &mut map);
                    }
                    map.insert(k, v);
                }
            }
        }
    }
    if let Some(cur) = out.last_mut() {
        flush(cur, &section, &mut map);
    }
    out
}

fn flush(cur: &mut VmInfo, section: &str, map: &mut BTreeMap<String, String>) {
    if map.is_empty() {
        return;
    }
    let g = |k: &str| map.get(k).cloned().unwrap_or_default();
    match section {
        "network-interface" => cur.nics.push(Nic {
            number: g("number"),
            emulation: g("emulation"),
            switch: g("virtual-switch"),
            mac: g("fixed-mac-address"),
            device: g("active-device"),
        }),
        "virtual-disk" => cur.disks.push(Disk {
            number: g("number"),
            device_type: g("device-type"),
            emulation: g("emulation"),
            path: g("system-path"),
            size: human_part(&g("bytes-size")),
            used: human_part(&g("bytes-used")),
        }),
        _ => {}
    }
    map.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "------------------------
Virtual Machine: qtumvalidator10
------------------------
  state: running (90212)
  datastore: default
  loader: uefi
  cpu: 2
  memory: 4096M

  console-ports
    com1: /dev/nmdm-qtumvalidator10.1B

  network-interface
    number: 0
    emulation: virtio-net
    virtual-switch: public
    fixed-mac-address: 58:9c:fc:07:ba:b0
    active-device: tap1

  virtual-disk
    number: 0
    device-type: file
    emulation: virtio-blk
    system-path: /zroot/vm/qtumvalidator10/disk0.img
    bytes-size: 85899345920 (80.000G)
    bytes-used: 915858432 (873.430M)

  virtual-disk
    number: 1
    device-type: custom
    emulation: ahci-cd
    system-path: /zroot/vm/.iso/seed.iso

------------------------
Virtual Machine: testvm
------------------------
  state: stopped
  loader: bhyveload
  cpu: 1
  memory: 256M

  network-interface
    number: 0
    emulation: virtio-net
    virtual-switch: public
";

    #[test]
    fn parses_multiple_guests() {
        let v = parse_info(SAMPLE);
        assert_eq!(v.len(), 2);
        let a = &v[0];
        assert_eq!(a.name, "qtumvalidator10");
        assert_eq!(a.state, VmState::Running(Some(90212)));
        assert_eq!(a.get("loader"), "uefi");
        assert_eq!(a.consoles, vec![("com1".into(), "/dev/nmdm-qtumvalidator10.1B".into())]);
        assert_eq!(a.nics.len(), 1);
        assert_eq!(a.nics[0].switch, "public");
        assert_eq!(a.nics[0].device, "tap1");
        assert_eq!(a.disks.len(), 2);
        assert_eq!(a.disks[0].size, "80.000G");
        assert_eq!(a.disks[1].emulation, "ahci-cd");
        let b = &v[1];
        assert_eq!(b.state, VmState::Stopped);
        assert_eq!(b.nics.len(), 1);
        assert!(b.disks.is_empty());
    }
}
