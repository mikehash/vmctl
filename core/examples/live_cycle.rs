//! Live smoke test of the write path against a real host (needs root/sudo).
//! Snapshots, rolls back, clones and destroys a STOPPED guest, then cleans up.
//! usage: live_cycle <stopped-guest>
use vmctl_core::inventory::{Hypervisor, LiveHost};
use vmctl_core::ops::Op;
fn main() {
    let name = std::env::args().nth(1).expect("guest name");
    let mut h = LiveHost::new();
    let inv = h.inventory();
    let g = inv.guest(&name).expect("guest exists");
    assert_eq!(g.state.kind(), "Stopped", "refusing: guest must be stopped");
    let ds = format!("{}/{}", "zroot/vm", name);
    let clone = format!("{name}-vmctl-smoke");
    let ops = vec![
        Op::Snapshot { name: name.clone(), snap: "vmctl-smoke".into(), force: false },
        Op::Rollback { name: name.clone(), snap: "vmctl-smoke".into(), dataset: ds.clone() },
        Op::Clone { name: name.clone(), snap: "vmctl-smoke".into(), to: clone.clone() },
        Op::Destroy(clone.clone()),
        Op::SnapDelete { name: name.clone(), snap: "vmctl-smoke".into(), dataset: ds },
    ];
    for op in ops {
        let r = h.run(op.program(), &op.argv());
        println!("exit {} · {}{}", r.code, r.cmd, if r.stderr.is_empty() { String::new() } else { format!("\n   stderr: {}", r.stderr) });
        if r.code != 0 { std::process::exit(1); }
        if let Op::Clone { .. } = op {
            let inv = h.inventory();
            println!("   clone visible in inventory: {}", inv.guest(&clone).is_some());
        }
    }
    let inv = h.inventory();
    println!("final: snaps on {name} = {}, clone gone = {}", inv.guest(&name).unwrap().snaps.len(), inv.guest(&clone).is_none());
}
