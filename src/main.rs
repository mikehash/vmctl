//! vmctl-tui — a friendly ratatui TUI for vm-bhyve.
//!
//! Foundation tick: the `vm(8)` wrapper + data model. TUI panes and the
//! assistant layer build on top of `vmbhyve` in later ticks.

mod vmbhyve;

fn main() {
    let vm = vmbhyve::Vm::default();
    match vm.list() {
        Ok(rows) => {
            println!("{:<12} {:<10} {:<10} {:<4} {:<8} {:<6} {}", "NAME", "DATASTORE", "LOADER", "CPU", "MEMORY", "AUTO", "STATE");
            for r in rows {
                println!(
                    "{:<12} {:<10} {:<10} {:<4} {:<8} {:<6} {}",
                    r.name, r.datastore, r.loader, r.cpu, r.memory, r.autostart, r.state
                );
            }
        }
        Err(e) => {
            eprintln!("vmctl-tui: {e}");
            std::process::exit(1);
        }
    }
}
