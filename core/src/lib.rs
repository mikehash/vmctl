//! vmctl-core — the shared engine behind both vmctl front-ends.
//!
//! A [`inventory::Hypervisor`] (live `vm(8)`/`zfs(8)` or the in-memory
//! [`demo::DemoHost`]) produces an [`inventory::Inventory`] and runs typed
//! [`ops::Op`]s with honest exit codes. On top of that: shared op `recipes`,
//! the plain-English `planner`, the background `worker`, and the shared
//! `activity` log.
//!
//! The front-ends (Slint desktop, ratatui TUI) are thin shells over this
//! crate, so anything you do in one shows up in the other.

pub mod activity;
pub mod backend;
pub mod demo;
pub mod info;
pub mod inventory;
pub mod iso;
pub mod model;
pub mod ops;
pub mod planner;
pub mod recipes;
pub mod templates;
pub mod vmbhyve;
pub mod worker;

pub use backend::ExecResult;
pub use iso::Iso;
pub use vmbhyve::{parse_list, VmSummary};
