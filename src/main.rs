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

const USAGE: &str = "\
vmctl-tui — a friendly TUI for vm-bhyve (VirtualBox-style)

USAGE:
  vmctl-tui                 Launch the interactive TUI (needs root for vm(8))
  vmctl-tui --list          Print the guest list and exit (scripting/smoke test)
  vmctl-tui --plan \"<text>\" Show + run a plain-English plan against live vm(8)
  vmctl-tui --help          Show this help

TUI PANES (hotkeys 1-6):
  1 Guests      VM list + action bar (Start/Stop/Restart/Suspend/Console/Info)
  2 Ask vmctl   Plain-English assistant: type a request, review the plan, run it
  3 New machine Creation wizard: template -> cpu/mem/disk -> vm create + install
  4 Config      Guest config surface + the exact edit command
  5 Console     The `vm console` command to run in a real shell
  6 Activity    Every real vm(8) command this session + its exit code

KEYS: 1-6 pane · ↑↓/jk move · ←→ pick action/template · Enter run · r refresh ·
      Esc back · q quit

Run as root (vm-bhyve requires it):  doas vmctl-tui";

fn main() {
    // `--help` / `-h` prints usage.
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }

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
