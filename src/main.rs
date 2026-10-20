//! vmctl-tui — a friendly ratatui TUI for vm-bhyve.
//!
//! Foundation: the `vm(8)` wrapper (`vmbhyve`) + actions + the VM-list TUI.
//! The creation wizard and assistant layer build on top in later ticks.

mod actions;
mod assistant;
mod backend;
mod panes;
mod ui;
mod vmbhyve;

use backend::Backend;

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

    // `--plan "<text>"` shows the assistant's validated plan for a request and
    // runs it against the live backend — proving the engine end-to-end on vm(8).
    if let Some(i) = std::env::args().position(|a| a == "--plan") {
        let text = std::env::args().nth(i + 1).unwrap_or_default();
        let mut backend = backend::VmBackend::default();
        let known: Vec<String> = backend
            .list()
            .map(|rows| rows.into_iter().map(|r| r.name).collect())
            .unwrap_or_default();
        let plan = assistant::plan_for(&text, &known);
        println!("PLAN: {}", plan.summary);
        for (n, s) in plan.steps.iter().enumerate() {
            println!("  {}. {}  [{}]", n + 1, s.title, s.cmd());
        }
        if plan.steps.is_empty() {
            return;
        }
        let report = assistant::run_plan(&mut backend, &plan);
        for r in &report.results {
            println!("  -> exit {}: {}", r.code, r.cmd);
            if !r.stderr.is_empty() {
                println!("     stderr: {}", r.stderr);
            }
        }
        match report.failed_at {
            Some(i) => {
                println!("STOPPED at step {} — nothing after it ran.", i + 1);
                std::process::exit(1);
            }
            None => println!("All {} step(s) succeeded.", report.results.len()),
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
