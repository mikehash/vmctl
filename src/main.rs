//! vmctl-tui — a friendly ratatui TUI for vm-bhyve.
//!
//! Foundation: the `vm(8)` wrapper (`vmbhyve`) + actions + the VM-list TUI.
//! The creation wizard and assistant layer build on top in later ticks.

mod actions;
mod ui;
mod vmbhyve;

use vmbhyve::Vm;

fn main() {
    // `--list` keeps a quick non-TUI path for scripting / smoke tests.
    if std::env::args().any(|a| a == "--list") {
        let vm = Vm::default();
        match vm.list() {
            Ok(rows) => {
                println!(
                    "{:<12} {:<10} {:<10} {:<4} {:<8} {:<6} {}",
                    "NAME", "DATASTORE", "LOADER", "CPU", "MEMORY", "AUTO", "STATE"
                );
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
        return;
    }

    let app = ui::App::new(Vm::default());
    let term = match ui::install_terminal() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("vmctl-tui: could not enter TUI: {e}");
            std::process::exit(1);
        }
    };
    // `ui::run` restores the terminal (raw mode + alternate screen) on both the
    // happy path and the error path before returning.
    if let Err(e) = ui::run(term, app) {
        eprintln!("vmctl-tui: {e}");
        std::process::exit(1);
    }
}
