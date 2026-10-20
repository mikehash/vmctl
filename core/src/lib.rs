//! vmctl-core — the shared engine behind both vmctl front-ends.
//!
//! Everything the TUI and the Desktop do lives here: the typed `vm(8)`
//! wrapper (`vmbhyve`), the high-level lifecycle `actions`, the plain-English
//! `assistant` (intent → validated plan → sequential runner), and the
//! `backend` abstraction (live `vm(8)` or an in-memory mock for tests).
//!
//! The front-ends are thin shells over this crate, so anything you do in one
//! shows up in the other — same core, same state, same honest exit codes.

pub mod actions;
pub mod assistant;
pub mod backend;
pub mod iso;
pub mod templates;
pub mod vmbhyve;

pub use actions::{suggested_action, Action};
pub use assistant::{plan_for, run_plan, Plan, RunReport, Step};
pub use backend::{Backend, ExecResult, MockBackend, VmBackend};
pub use iso::{discover as discover_isos, Iso};
pub use vmbhyve::{parse_list, Vm, VmError, VmSummary};
