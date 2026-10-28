//! Background worker: the only thread that talks to the hypervisor.
//!
//! N-103: no front-end blocks on vm(8). Jobs are queued over a channel and run
//! strictly in order (vm-bhyve isn't safe to drive concurrently anyway). Every
//! result goes through the caller's `post` callback: the desktop wraps it in
//! Slint's `invoke_from_event_loop`; the TUI forwards into an mpsc channel it
//! drains in its event loop (see [`Worker::spawn_channel`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::backend::ExecResult;
use crate::inventory::{Hypervisor, Inventory};
use crate::ops::Op;

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

    /// Same worker, but events arrive on a `Receiver` the caller polls
    /// (for front-ends with their own event loop, e.g. the TUI).
    pub fn spawn_channel(hv: Box<dyn Hypervisor>) -> (Self, Receiver<Event>) {
        let (etx, erx) = channel::<Event>();
        let w = Self::spawn(hv, move |ev| {
            let _ = etx.send(ev);
        });
        (w, erx)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoHost;
    use crate::model::VmState;
    use std::time::Duration;

    fn drain_until_idle(rx: &Receiver<Event>, busy_cycles: usize) -> Vec<Event> {
        let mut out = Vec::new();
        let mut done = 0;
        while done < busy_cycles {
            let ev = rx.recv_timeout(Duration::from_secs(10)).expect("worker event");
            if matches!(ev, Event::Busy(false)) {
                done += 1;
            }
            out.push(ev);
        }
        out
    }

    #[test]
    fn refresh_posts_inventory_between_busy_flags() {
        let (w, rx) = Worker::spawn_channel(Box::new(DemoHost::new()));
        w.refresh();
        let ev = drain_until_idle(&rx, 1);
        assert!(matches!(ev.first(), Some(Event::Busy(true))));
        assert!(matches!(ev.get(1), Some(Event::Inventory(inv)) if !inv.guests.is_empty()));
        assert!(matches!(ev.last(), Some(Event::Busy(false))));
    }

    #[test]
    fn run_streams_steps_in_order_then_refreshes() {
        let (w, rx) = Worker::spawn_channel(Box::new(DemoHost::new()));
        w.run(vec![Op::Start("win11-dev".into()), Op::Stop("win11-dev".into())], Tag::Action);
        let ev = drain_until_idle(&rx, 1);
        let steps: Vec<i32> = ev.iter().filter_map(|e| if let Event::Step { result, .. } = e { Some(result.code) } else { None }).collect();
        assert_eq!(steps, [0, 0]);
        assert!(ev.iter().any(|e| matches!(e, Event::RunDone { failed_at: None, tag: Tag::Action })));
        assert!(ev.iter().any(|e| matches!(e, Event::Inventory(_))));
    }

    #[test]
    fn run_stops_at_first_failure() {
        let (w, rx) = Worker::spawn_channel(Box::new(DemoHost::new()));
        // win11-dev is stopped in the demo: stop fails, start must not run.
        w.run(vec![Op::Stop("win11-dev".into()), Op::Start("win11-dev".into())], Tag::Plan(7));
        let ev = drain_until_idle(&rx, 1);
        let steps = ev.iter().filter(|e| matches!(e, Event::Step { .. })).count();
        assert_eq!(steps, 1);
        assert!(ev.iter().any(|e| matches!(e, Event::RunDone { failed_at: Some(0), tag: Tag::Plan(7) })));
        let inv = ev.iter().find_map(|e| if let Event::Inventory(i) = e { Some(i) } else { None }).unwrap();
        assert_eq!(inv.guest("win11-dev").unwrap().state, VmState::Stopped);
    }

    /// Demo host whose `run` takes a while, so jobs deterministically queue behind it.
    struct Slow(DemoHost);
    impl Hypervisor for Slow {
        fn run(&mut self, program: &str, argv: &[String]) -> ExecResult {
            std::thread::sleep(Duration::from_millis(300));
            self.0.run(program, argv)
        }
        fn inventory(&mut self) -> Inventory {
            self.0.inventory()
        }
        fn console_argv(&self, name: &str) -> Vec<String> {
            self.0.console_argv(name)
        }
    }

    #[test]
    fn queued_refreshes_coalesce() {
        let (w, rx) = Worker::spawn_channel(Box::new(Slow(DemoHost::new())));
        // Hold the worker busy with a run so refreshes queue behind it.
        w.run(vec![Op::Start("win11-dev".into())], Tag::Action);
        for _ in 0..20 {
            w.refresh();
        }
        // run (1 idle) + at most one coalesced refresh (+1 idle)
        let _ = drain_until_idle(&rx, 2);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "extra refreshes leaked through");
    }
}
