//! Print what LiveHost sees: `cargo run -p vmctl-core --example inventory`
use vmctl_core::inventory::{Hypervisor, LiveHost};
fn main() {
    let mut h = LiveHost::new();
    let inv = h.inventory();
    println!("{:?} {} {} {} thr {} {}MiB suspend={} list={}ms err={:?}", inv.privilege, inv.host.hostname, inv.host.os, inv.host.vm_version, inv.host.threads, inv.host.mem_mib, inv.host.suspend_supported, inv.list_ms, inv.error);
    for g in &inv.guests {
        println!("  {} {} {}cpu {}MiB {} sw={:?} con={} snaps={} disks={}", g.name, g.state.label(), g.cpu, g.mem_mib, g.loader, g.switches(), g.console(), g.snaps.len(), g.info.as_ref().map(|i| i.disks.len()).unwrap_or(0));
    }
    for s in &inv.switches { println!("  switch {:?}", s); }
    for d in &inv.datastores { println!("  ds {} {} used {:.0}%", d.name, d.path, d.used_pct()); }
    for i in &inv.isos { println!("  iso {} {}", i.name, i.size_h()); }
    println!("  templates {:?}", inv.templates.iter().map(|t| t.id.as_str()).collect::<Vec<_>>());
}
