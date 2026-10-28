//! vmctl-tui — Mission Control for vm-bhyve, in a terminal.
//!
//! Same v2 core as the desktop: `Inventory` from a `Hypervisor` (live or demo),
//! typed `Op`s with predictive guards, the shared recipes, the planner, and the
//! shared activity log. The hypervisor lives on the core worker thread; the UI
//! thread never blocks on vm(8).

mod app;
mod theme;
mod view;

use std::io;
use std::process::Command;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as CtEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use vmctl_core::activity::{self, ActivityLog, Entry};
use vmctl_core::demo::DemoHost;
use vmctl_core::inventory::{Hypervisor, LiveHost, Privilege};
use vmctl_core::model::fmt_mib;
use vmctl_core::planner;
use vmctl_core::worker::{Event, Worker};

use app::{App, Effect};

const USAGE: &str = "\
vmctl-tui — Mission Control for vm-bhyve, in a terminal

USAGE:
  vmctl-tui                  Interactive TUI against this host's vm-bhyve
  vmctl-tui --demo           Interactive TUI against a built-in demo host
  vmctl-tui --list           Print the guest inventory and exit (read-only)
  vmctl-tui --plan \"<text>\"  Print the assistant's plan for a request (never runs it)
  vmctl-tui --help           This help

Add --demo to --list / --plan to use the demo host.

VIEWS (Tab or 1-6): Machines · Snapshots · Network · Storage · Activity · Ask vmctl
Press ? inside the TUI for keys.

Privilege: runs vm(8) directly as root, or via `sudo -n` when that works
without a password. Otherwise the TUI is read-only and says so.
Activity is shared with vmctl-desktop (~/.local/state/vmctl/activity.log).";

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn host(demo: bool) -> Box<dyn Hypervisor> {
    if demo {
        Box::new(DemoHost::new())
    } else {
        Box::new(LiveHost::new())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let demo = has("--demo");
    if has("-h") || has("--help") {
        println!("{USAGE}");
        return;
    }
    if let Some(a) = args.iter().find(|a| a.starts_with('-') && !["--demo", "--list", "--plan"].contains(&a.as_str())) {
        eprintln!("vmctl-tui: unknown option {a}\n\n{USAGE}");
        std::process::exit(2);
    }
    if has("--list") {
        std::process::exit(list(host(demo).as_mut()));
    }
    if let Some(i) = args.iter().position(|a| a == "--plan") {
        let text = args.get(i + 1).cloned().unwrap_or_default();
        std::process::exit(plan(host(demo).as_mut(), &text));
    }

    let log = if demo { ActivityLog::memory() } else { ActivityLog::open(activity::default_path()) };
    let app = App::new(log, demo);
    let (worker, rx) = Worker::spawn_channel(host(demo));
    let term = match install_terminal() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("vmctl-tui: could not enter TUI: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = run(term, app, &worker, &rx) {
        eprintln!("vmctl-tui: {e}");
        std::process::exit(1);
    }
}

/// `--list`: one inventory pass, plain table. Exit 1 if the inventory is partial.
fn list(hv: &mut dyn Hypervisor) -> i32 {
    let inv = hv.inventory();
    println!("{:<18} {:<10} {:<10} {:>4} {:>7} {:<5} {:<20} SNAPS", "NAME", "DATASTORE", "LOADER", "CPU", "MEMORY", "AUTO", "STATE");
    for g in &inv.guests {
        println!(
            "{:<18} {:<10} {:<10} {:>4} {:>7} {:<5} {:<20} {}",
            g.name,
            g.datastore,
            g.loader,
            g.cpu,
            fmt_mib(g.mem_mib),
            if g.autostart { "yes" } else { "no" },
            g.state.label(),
            g.snaps.len()
        );
    }
    let p = match inv.privilege {
        Privilege::Root => "root",
        Privilege::Sudo => "sudo",
        Privilege::None => "read-only",
    };
    println!("\n{} guests · {} switches · {} datastores · {} isos · {} · vm list {}ms", inv.guests.len(), inv.switches.len(), inv.datastores.len(), inv.isos.len(), p, inv.list_ms);
    match inv.error {
        Some(e) => {
            eprintln!("vmctl-tui: inventory is partial: {e}");
            1
        }
        None => 0,
    }
}

/// `--plan`: print what the assistant would do. Never runs anything — the
/// interactive TUI is where a plan is reviewed and run.
fn plan(hv: &mut dyn Hypervisor, text: &str) -> i32 {
    if text.trim().is_empty() {
        eprintln!("vmctl-tui: --plan needs a request, e.g. --plan \"snapshot everything that's running\"");
        return 2;
    }
    let inv = hv.inventory();
    let log = ActivityLog::open(activity::default_path());
    let r = planner::plan(text, &inv, None, &log.entries);
    println!("{}", r.text);
    if let Some(e) = &r.stderr {
        println!("stderr: {e}");
    }
    for (i, s) in r.steps.iter().enumerate() {
        println!("  {}. {:<32} $ {}", i + 1, s.title, s.op.cmd());
    }
    for s in &r.skipped {
        println!("  skipped: {s}");
    }
    for c in &r.code {
        println!("  $ {c}");
    }
    if r.has_plan() {
        println!("risk: {} · touches: {}  (not run — open the TUI to run it)", r.risk, r.touches);
    }
    0
}

fn install_terminal() -> io::Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(e) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(e);
    }
    Terminal::new(CrosstermBackend::new(stdout))
}

fn leave_terminal(term: &mut Term) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()
}

fn enter_terminal(term: &mut Term) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(term.backend_mut(), EnterAlternateScreen)?;
    term.clear()
}

/// Run until quit; always restores the terminal, including on error.
fn run(mut term: Term, mut app: App, worker: &Worker, rx: &Receiver<Event>) -> io::Result<()> {
    let r = event_loop(&mut term, &mut app, worker, rx);
    let restore = leave_terminal(&mut term);
    r.and(restore)
}

const AUTO_REFRESH: Duration = Duration::from_secs(5);

fn event_loop(term: &mut Term, app: &mut App, worker: &Worker, rx: &Receiver<Event>) -> io::Result<()> {
    // First inventory now; the auto-refresh timer only covers later ones.
    worker.refresh();
    let mut last_refresh = Instant::now();
    loop {
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, Event::Inventory(_)) {
                last_refresh = Instant::now();
            }
            app.on_event(ev);
        }
        term.draw(|f| view::draw(f, app))?;
        if last_refresh.elapsed() >= AUTO_REFRESH && !app.busy {
            worker.refresh();
            last_refresh = Instant::now();
        }
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let CtEvent::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match app.on_key(key) {
            None => {}
            Some(Effect::Quit) => return Ok(()),
            Some(Effect::Refresh) => worker.refresh(),
            Some(Effect::Reprobe) => worker.reprobe(),
            Some(Effect::Run { ops, tag }) => worker.run(ops, tag),
            Some(Effect::Console(argv)) => {
                let status = console(term, &argv)?;
                if !app.demo {
                    let r = vmctl_core::backend::ExecResult {
                        cmd: argv.join(" "),
                        code: status.as_ref().map(|s| s.code().unwrap_or(-1)).unwrap_or(127),
                        stdout: String::new(),
                        stderr: status.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
                    };
                    let _ = app.log.push(Entry::from_result(app::SOURCE, &r));
                }
                app.console_closed(&argv, status);
                worker.refresh();
            }
        }
    }
}

/// Hand the real terminal to `vm console` (cu), then take it back.
fn console(term: &mut Term, argv: &[String]) -> io::Result<io::Result<std::process::ExitStatus>> {
    leave_terminal(term)?;
    println!("vmctl: attaching `{}` — detach with ~. (tilde, dot) on a new line.", argv.join(" "));
    let status = match argv.split_first() {
        Some((p, rest)) => Command::new(p).args(rest).status(),
        None => Err(io::Error::new(io::ErrorKind::InvalidInput, "empty console command")),
    };
    enter_terminal(term)?;
    Ok(status)
}
