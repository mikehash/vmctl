# TUI port onto the v2 core (branch tui-port, base desktop-rebuild @ 7d0fba2)

Substrate (walked 2026-10-28):
- TUI (tui/src: ui.rs 783, panes.rs 823, panes2.rs 224, theme.rs, main.rs) still drives the
  v1 API: vmbhyve::Vm (blocking, on the UI thread), assistant::{plan_for,run_plan},
  backend::VmBackend, iso::scan_dir. Feature panes scrape raw `vm` text into Vec<String>.
  Snapshots pane greps `vm list -v` for '@' (wrong source; core reads zfs).
- Desktop uses v2: inventory::{Hypervisor, LiveHost, Inventory}, demo::DemoHost, ops::Op
  (+guard/undo/title/cmd), planner::{plan, run_steps, explain_stderr}, activity::ActivityLog,
  ops::suggest. Worker thread in desktop/src/worker.rs (pure std mpsc, no Slint).
- UI-agnostic op recipes live in desktop/src/main.rs: op_for, confirm_ops, wizard_ops (+WizSpec),
  dataset_of. The TUI needs the same recipes -> they must move to core.

Cuts:
1. core::recipes — move op_for/confirm_ops/wizard_ops/WizSpec/dataset_of out of desktop main.rs,
   add tests (rollback fans out per dataset, snapdelete path, wizard order create->switch->autostart->install).
   Desktop calls core. Gate: core tests + desktop tests + clippy.
2. core::worker — move desktop/src/worker.rs into core (post callback is generic). Desktop
   wraps post in invoke_from_event_loop; TUI drains an mpsc Receiver in its event loop.
3. TUI rewrite over Inventory: guest list w/ state pills from VmState, detail (info/nics/disks/
   snaps/suggest), actions via Op + guard (greyed reasons), confirm modal for is_destructive()
   with snapshot-first, wizard from templates/isos/switches/datastores, assistant via planner
   with step-by-step run + real stderr, Snapshots/Switches/Datastores/Images from Inventory,
   Activity pane from ActivityLog (shared file with desktop), --demo flag, privilege banner.
   Console: suspend TUI, exec console_argv, restore.
4. Retire v1 (assistant.rs, actions.rs, backend::VmBackend users) once nothing imports them.
5. Gates: cargo test (root ws + desktop ws), clippy -D warnings both, `vmctl-tui --demo` render
   check via ratatui TestBackend snapshot tests, live read-only smoke (`--list`) on this host.

Status (2026-10-28): cuts 1-5 done on tui-port.
- 3d24527 cut 3: tui/src = app.rs (pure state machine → Effects) + view.rs (render) + main.rs
  (worker loop, --demo/--list/--plan, console suspend/exec/restore). --plan now prints only.
- 8c6cf4a cut 4: core::assistant + core::actions deleted; backend.rs = ExecResult only;
  vmbhyve.rs = parse_list only; anyhow/thiserror dropped.
- cut 5 gates: cargo test root ws (57 core + 21 tui) + desktop ws (3); clippy -D warnings both;
  TestBackend goldens in tui/tests/snapshots (UPDATE_SNAPSHOTS=1); live read-only --list on
  this host (4 guests, sudo, exit 0) + PTY launch/view-cycle/quit of the release binary.
