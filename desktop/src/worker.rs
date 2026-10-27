//! Background worker: the only thread that talks to the hypervisor.
//!
//! N-103: the GUI never blocks on vm(8). Jobs are queued over a channel and run
//! strictly in order (vm-bhyve isn't safe to drive concurrently anyway). Every
//! result is posted back to the UI thread with `invoke_from_event_loop`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

use vmctl_core::backend::ExecResult;
use vmctl_core::inventory::{Hypervisor, Inventory};
use vmctl_core::ops::Op;

/// What the UI asked for.
pub enum Job {
    Refresh,
    /// Re-detect root/sudo, then refresh.
    Reprobe,
    /// Run ops in order, stop at the first non-zero exit, then refresh.
    Run { ops: Vec<Op>, tag: Tag },
}

/// Who asked for a run, so results land in the right place.
#[derive(Clone, Debug)]
pub enum Tag {
    /// One-click action: show a toast with the last result.
    Action,
    /// Assistant plan message id: update its step list live.
    Plan(i32),
    /// Wizard: select the new guest when done.
    Create(String),
    /// Clone: select the clone when done.
    Clone(String),
}

/// What the worker reports back.
pub enum Event {
    Inventory(Box<Inventory>),
    Step { tag: Tag, result: ExecResult },
    RunDone { tag: Tag, failed_at: Option<usize> },
    Busy(bool),
}

#[derive(Clone)]
pub struct Worker {
    tx: Sender<Job>,
    refresh_pending: Arc<AtomicBool>,
}

impl Worker {
    pub fn spawn(mut hv: Box<dyn Hypervisor>, post: impl Fn(Event) + Send + 'static) -> Self {
        let (tx, rx) = channel::<Job>();
        let pending = Arc::new(AtomicBool::new(false));
        let p2 = pending.clone();
        std::thread::Builder::new()
            .name("vmctl-worker".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Refresh => {
                            p2.store(false, Ordering::SeqCst);
                            post(Event::Busy(true));
                            post(Event::Inventory(Box::new(hv.inventory())));
                            post(Event::Busy(false));
                        }
                        Job::Reprobe => {
                            hv.reprobe();
                            post(Event::Inventory(Box::new(hv.inventory())));
                        }
                        Job::Run { ops, tag } => {
                            post(Event::Busy(true));
                            let mut failed_at = None;
                            for (i, op) in ops.iter().enumerate() {
                                let r = hv.run(op.program(), &op.argv());
                                let failed = r.code != 0;
                                post(Event::Step { tag: tag.clone(), result: r });
                                if failed {
                                    failed_at = Some(i);
                                    break;
                                }
                            }
                            post(Event::RunDone { tag, failed_at });
                            post(Event::Inventory(Box::new(hv.inventory())));
                            post(Event::Busy(false));
                        }
                    }
                }
            })
            .expect("spawn worker thread");
        Self { tx, refresh_pending: pending }
    }

    /// Queue a refresh unless one is already waiting (timer ticks don't pile up).
    pub fn refresh(&self) {
        if !self.refresh_pending.swap(true, Ordering::SeqCst) {
            let _ = self.tx.send(Job::Refresh);
        }
    }

    pub fn reprobe(&self) {
        let _ = self.tx.send(Job::Reprobe);
    }

    pub fn run(&self, ops: Vec<Op>, tag: Tag) {
        let _ = self.tx.send(Job::Run { ops, tag });
    }
}
