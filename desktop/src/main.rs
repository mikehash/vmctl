//! vmctl-desktop — the Slint "Mission Control" front-end over `vmctl-core`.
//!
//! The UI is declared in `ui/vmctl.slint`; this file wires it to the shared
//! core so the desktop drives the same `vm(8)` backend as the TUI. Anything
//! you do here shows up in the TUI and vice-versa.

slint::include_modules!();

use slint::{Model, ModelRc, VecModel};
use std::rc::Rc;
use vmctl_core::vmbhyve::Vm;

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;

    // Seed the VM list from the live backend (best-effort; the UI still works
    // read-only if `vm list` fails, e.g. without root).
    let vm = Vm::default();
    if let Ok(rows) = vm.list() {
        let data: Vec<VmRowData> = rows
            .iter()
            .map(|r| VmRowData {
                name: r.name.clone().into(),
                os: r.loader.clone().into(),
                state: r.state.clone().into(),
                cpu: r.cpu.parse().unwrap_or(0),
                mem: r.memory.clone().into(),
            })
            .collect();
        ui.set_vms(ModelRc::new(VecModel::from(data)));
    }

    // Start the selected guest via the shared core.
    let vm_start = Vm::default();
    let ui_weak = ui.as_weak();
    ui.on_start_selected(move || {
        let Some(ui) = ui_weak.upgrade() else { return };
        let idx = ui.get_selected() as usize;
        let vms = ui.get_vms();
        if let Some(row) = vms.row_data(idx) {
            let name = row.name.to_string();
            match vm_start.run(&["start", &name]) {
                Ok(msg) => ui.set_status(msg.into()),
                Err(e) => ui.set_status(format!("error: {e}").into()),
            }
        }
    });

    // Stop the selected guest via the shared core.
    let vm_stop = Vm::default();
    let ui_weak = ui.as_weak();
    ui.on_stop_selected(move || {
        let Some(ui) = ui_weak.upgrade() else { return };
        let idx = ui.get_selected() as usize;
        let vms = ui.get_vms();
        if let Some(row) = vms.row_data(idx) {
            let name = row.name.to_string();
            match vm_stop.run(&["stop", &name]) {
                Ok(msg) => ui.set_status(msg.into()),
                Err(e) => ui.set_status(format!("error: {e}").into()),
            }
        }
    });

    // Assistant: plain English → validated plan (via the shared assistant).
    let ui_weak = ui.as_weak();
    ui.on_ask(move |text| {
        let Some(ui) = ui_weak.upgrade() else { return };
        let t = text.to_string();
        if t.trim().is_empty() {
            return;
        }
        let vms = ui.get_vms();
        let known: Vec<String> = (0..vms.row_count()).filter_map(|i| vms.row_data(i)).map(|v| v.name.to_string()).collect();
        let plan = vmctl_core::assistant::plan_for(&t, &known);
        let tmodel = ui.get_transcript();
        let mut tr: Vec<slint::SharedString> =
            (0..tmodel.row_count()).filter_map(|i| tmodel.row_data(i)).collect();
        tr.push(format!("> {t}").into());
        tr.push(format!("  plan: {}", plan.summary).into());
        for s in &plan.steps {
            tr.push(format!("    - {} ({})", s.cmd(), s.effect).into());
        }
        ui.set_transcript(ModelRc::new(VecModel::from(tr)));
        ui.set_status(format!("plan: {} step(s) · risk {}", plan.steps.len(), plan.risk).into());
    });

    ui.run()
}
