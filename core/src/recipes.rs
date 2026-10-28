//! UI-agnostic op recipes: turn "the user clicked X on guest Y" into the exact
//! `Op` sequence to plan and run. Shared by the desktop and the TUI so both
//! front-ends issue identical command lines for the same intent.

use crate::inventory::{Guest, Inventory};
use crate::model::{now, stamp};
use crate::ops::Op;

/// Fallback ZFS dataset when a guest's datastore isn't in the inventory.
pub const DEFAULT_DATASET: &str = "zroot/vm";
/// Fallback datastore mount path, paired with [`DEFAULT_DATASET`].
pub const DEFAULT_DS_PATH: &str = "/zroot/vm";

/// Look up a single (non-confirmed) op by its UI name. `None` for unknown
/// names, or for ops that need guest state (autostart/unlock) when `g` is
/// missing.
pub fn op_for(op: &str, name: &str, g: Option<&Guest>) -> Option<Op> {
    let n = name.to_string();
    Some(match op {
        "start" => Op::Start(n),
        "stop" => Op::Stop(n),
        "restart" => Op::Restart(n),
        "suspend" => Op::Suspend(n),
        "poweroff" => Op::Poweroff(n),
        "destroy" => Op::Destroy(n),
        "snapshot" => Op::Snapshot { force: g.map(|g| g.state.is_running()).unwrap_or(false), snap: format!("manual-{}", stamp(now())), name: n },
        "autostart" => Op::Autostart { on: !g?.autostart, name: n },
        "unlock" => Op::Unlock { lock: g?.dir.join("run.lock"), name: n },
        _ => return None,
    })
}

/// The datastore's ZFS dataset a guest lives under (parent of the guest dataset).
pub fn dataset_of(g: &Guest, inv: &Inventory) -> String {
    inv.datastore(&g.datastore).map(|d| d.dataset.clone()).unwrap_or_else(|| DEFAULT_DATASET.into())
}

/// Ops behind a confirm dialog. `safety` takes a snapshot before a hard
/// poweroff. Rollback fans out to every dataset carrying `@snap` (guest fs +
/// zvols) so multi-disk guests roll back consistently.
pub fn confirm_ops(inv: &Inventory, kind: &str, name: &str, snap: &str, safety: bool) -> Vec<Op> {
    let g = inv.guest(name);
    let mut v = Vec::new();
    match kind {
        "poweroff" => {
            if safety {
                v.push(Op::Snapshot { name: name.into(), snap: format!("pre-poweroff-{}", stamp(now())), force: true });
            }
            v.push(Op::Poweroff(name.into()));
        }
        "rollback" => {
            if let Some(g) = g {
                let mut ds: Vec<String> = g.snaps.iter().find(|s| s.name == snap).map(|s| s.datasets.clone()).unwrap_or_default();
                if ds.is_empty() {
                    ds.push(format!("{}/{name}", dataset_of(g, inv)));
                }
                for dataset in ds {
                    v.push(Op::Rollback { name: name.into(), snap: snap.into(), dataset });
                }
            }
        }
        "destroy" => v.push(Op::Destroy(name.into())),
        "snapdelete" => {
            if let Some(g) = g {
                v.push(Op::SnapDelete { name: name.into(), snap: snap.into(), dataset: format!("{}/{name}", dataset_of(g, inv)) });
            }
        }
        "unlock" => {
            if let Some(g) = g {
                v.push(Op::Unlock { name: name.into(), lock: g.dir.join("run.lock") });
            }
        }
        _ => {}
    }
    v
}

/// What the create wizard collected. Sizes are whole GiB; zeros are clamped to 1.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WizSpec {
    pub tpl: String,
    pub cpu: u32,
    pub mem_gib: u32,
    pub disk_gib: u32,
    pub ds: String,
    pub sw: String,
    pub iso: String,
    pub name: String,
    pub install: bool,
    pub autostart: bool,
}

/// Create-wizard recipe, in run order: create → set switch (only if it
/// differs from the template's) → autostart → install.
pub fn wizard_ops(inv: &Inventory, spec: &WizSpec) -> Vec<Op> {
    let name = spec.name.clone();
    let mut v = vec![Op::Create {
        name: name.clone(),
        template: spec.tpl.clone(),
        datastore: spec.ds.clone(),
        disk_gb: spec.disk_gib.max(1),
        cpu: spec.cpu.max(1),
        mem_mib: u64::from(spec.mem_gib.max(1)) * 1024,
    }];
    let tpl_switch = inv.templates.iter().find(|t| t.id == spec.tpl).map(|t| t.switch.clone()).unwrap_or_default();
    if !spec.sw.is_empty() && spec.sw != tpl_switch {
        let ds = inv.datastore(&spec.ds).map(|d| d.path.clone()).unwrap_or_else(|| DEFAULT_DS_PATH.into());
        v.push(Op::SetSwitch { name: name.clone(), conf: format!("{ds}/{name}/{name}.conf").into(), switch: spec.sw.clone() });
    }
    if spec.autostart {
        v.push(Op::Autostart { name: name.clone(), on: true });
    }
    if spec.install && !spec.iso.is_empty() {
        // Full path for ISOs outside the datastore's .iso folder (vm install accepts both).
        let iso = inv
            .isos
            .iter()
            .find(|i| i.name == spec.iso)
            .map(|i| if i.source == "datastore" { i.name.clone() } else { i.path.display().to_string() })
            .unwrap_or_else(|| spec.iso.clone());
        v.push(Op::Install { name, iso });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoHost;
    use crate::inventory::Hypervisor;
    use crate::iso::Iso;
    use std::path::PathBuf;

    fn inv() -> Inventory {
        DemoHost::new().inventory()
    }

    #[test]
    fn op_for_basic_and_unknown() {
        let inv = inv();
        assert_eq!(op_for("start", "x", None), Some(Op::Start("x".into())));
        assert_eq!(op_for("bogus", "x", None), None);
        // autostart/unlock need guest state
        assert_eq!(op_for("autostart", "x", None), None);
        assert_eq!(op_for("unlock", "x", None), None);
        let g = inv.guest("ubuntu-2404").unwrap();
        assert_eq!(op_for("autostart", "ubuntu-2404", Some(g)), Some(Op::Autostart { name: "ubuntu-2404".into(), on: !g.autostart }));
        assert_eq!(op_for("unlock", "ubuntu-2404", Some(g)), Some(Op::Unlock { name: "ubuntu-2404".into(), lock: g.dir.join("run.lock") }));
    }

    #[test]
    fn snapshot_forces_on_running_guest_only() {
        let inv = inv();
        let run = op_for("snapshot", "ubuntu-2404", inv.guest("ubuntu-2404")).unwrap();
        let stop = op_for("snapshot", "win11-dev", inv.guest("win11-dev")).unwrap();
        assert!(matches!(run, Op::Snapshot { force: true, ref snap, .. } if snap.starts_with("manual-")));
        assert!(matches!(stop, Op::Snapshot { force: false, .. }));
    }

    #[test]
    fn dataset_of_uses_datastore_or_default() {
        let inv = inv();
        assert_eq!(dataset_of(inv.guest("win11-dev").unwrap(), &inv), "nvme/vm");
        let mut g = inv.guest("win11-dev").unwrap().clone();
        g.datastore = "nope".into();
        assert_eq!(dataset_of(&g, &inv), DEFAULT_DATASET);
    }

    #[test]
    fn rollback_fans_out_per_dataset() {
        let mut inv = inv();
        let g = inv.guests.iter_mut().find(|g| g.name == "win11-dev").unwrap();
        g.snaps[0].datasets = vec!["nvme/vm/win11-dev".into(), "nvme/vm/win11-dev/disk0".into(), "nvme/vm/win11-dev/disk1".into()];
        let ops = confirm_ops(&inv, "rollback", "win11-dev", "activated", false);
        let ds: Vec<_> = ops
            .iter()
            .map(|o| match o {
                Op::Rollback { dataset, snap, .. } => {
                    assert_eq!(snap, "activated");
                    dataset.as_str()
                }
                o => panic!("unexpected {o:?}"),
            })
            .collect();
        assert_eq!(ds, ["nvme/vm/win11-dev", "nvme/vm/win11-dev/disk0", "nvme/vm/win11-dev/disk1"]);
    }

    #[test]
    fn rollback_unknown_snap_falls_back_to_guest_dataset() {
        let inv = inv();
        let ops = confirm_ops(&inv, "rollback", "win11-dev", "missing", false);
        assert_eq!(ops, vec![Op::Rollback { name: "win11-dev".into(), snap: "missing".into(), dataset: "nvme/vm/win11-dev".into() }]);
        assert!(confirm_ops(&inv, "rollback", "ghost", "x", false).is_empty());
    }

    #[test]
    fn snapdelete_targets_guest_dataset() {
        let inv = inv();
        let ops = confirm_ops(&inv, "snapdelete", "ubuntu-2404", "pre-upgrade", false);
        assert_eq!(ops, vec![Op::SnapDelete { name: "ubuntu-2404".into(), snap: "pre-upgrade".into(), dataset: "zroot/vm/ubuntu-2404".into() }]);
    }

    #[test]
    fn poweroff_safety_snapshot_comes_first() {
        let inv = inv();
        let ops = confirm_ops(&inv, "poweroff", "ubuntu-2404", "", true);
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Op::Snapshot { force: true, snap, .. } if snap.starts_with("pre-poweroff-")));
        assert_eq!(ops[1], Op::Poweroff("ubuntu-2404".into()));
        assert_eq!(confirm_ops(&inv, "poweroff", "ubuntu-2404", "", false), vec![Op::Poweroff("ubuntu-2404".into())]);
        assert_eq!(confirm_ops(&inv, "destroy", "x", "", false), vec![Op::Destroy("x".into())]);
        assert!(confirm_ops(&inv, "nope", "x", "", false).is_empty());
    }

    fn spec(inv: &Inventory) -> WizSpec {
        let tpl = inv.templates.first().expect("demo has templates");
        WizSpec { tpl: tpl.id.clone(), cpu: 2, mem_gib: 4, disk_gib: 20, ds: "fast".into(), sw: tpl.switch.clone(), iso: String::new(), name: "newvm".into(), install: false, autostart: false }
    }

    #[test]
    fn wizard_minimal_is_create_only_and_clamps_zeros() {
        let inv = inv();
        let mut s = spec(&inv);
        s.cpu = 0;
        s.mem_gib = 0;
        s.disk_gib = 0;
        let ops = wizard_ops(&inv, &s);
        assert_eq!(ops, vec![Op::Create { name: "newvm".into(), template: s.tpl.clone(), datastore: "fast".into(), disk_gb: 1, cpu: 1, mem_mib: 1024 }]);
    }

    #[test]
    fn wizard_order_create_switch_autostart_install() {
        let mut inv = inv();
        inv.isos.push(Iso { name: "ext.iso".into(), path: PathBuf::from("/home/u/ext.iso"), size: 1, source: "home" });
        let mut s = spec(&inv);
        s.sw = "nat0".into();
        s.autostart = true;
        s.install = true;
        s.iso = "ext.iso".into();
        let ops = wizard_ops(&inv, &s);
        let kinds: Vec<&str> = ops
            .iter()
            .map(|o| match o {
                Op::Create { .. } => "create",
                Op::SetSwitch { .. } => "switch",
                Op::Autostart { on: true, .. } => "autostart",
                Op::Install { .. } => "install",
                o => panic!("unexpected {o:?}"),
            })
            .collect();
        assert_eq!(kinds, ["create", "switch", "autostart", "install"]);
        assert_eq!(ops[1], Op::SetSwitch { name: "newvm".into(), conf: "/nvme/vm/newvm/newvm.conf".into(), switch: "nat0".into() });
        // non-datastore ISO → full path
        assert_eq!(ops[3], Op::Install { name: "newvm".into(), iso: "/home/u/ext.iso".into() });
    }

    #[test]
    fn wizard_datastore_iso_by_name_and_install_needs_iso() {
        let inv = inv();
        let iso = inv.isos.iter().find(|i| i.source == "datastore").expect("demo has datastore isos").name.clone();
        let mut s = spec(&inv);
        s.install = true;
        s.iso = iso.clone();
        assert_eq!(wizard_ops(&inv, &s).last(), Some(&Op::Install { name: "newvm".into(), iso }));
        s.iso.clear();
        assert_eq!(wizard_ops(&inv, &s).len(), 1);
    }
}
