use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use freya::prelude::{WritableUtils, spawn_forever};

use super::ui::{Dialog, Ui};
use crate::progress::{self, Progress, Report};
use crate::session;

// One task runs at a time, so one flag tells it to stop.
static STOP: AtomicBool = AtomicBool::new(false);

pub fn run<T: Send + 'static>(
    ui: Ui,
    label: &str,
    work: impl FnOnce(Report) -> anyhow::Result<T> + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    start(ui, label, false, work, done);
}

pub fn run_stoppable<T: Send + 'static>(
    ui: Ui,
    label: &str,
    work: impl FnOnce(Report) -> anyhow::Result<T> + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    start(ui, label, true, work, done);
}

/// One task runs at a time; a second one is refused aloud rather than dropped.
#[must_use]
pub fn reject_busy(ui: Ui) -> bool {
    let Some(running) = ui.busy.peek().clone() else {
        return false;
    };
    let mut dialog = ui.dialog;
    dialog.set(Dialog::Notice(format!(
        "ufk is busy: {}. Try again when it ends.",
        running.step
    )));
    true
}

pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
}

fn start<T: Send + 'static>(
    ui: Ui,
    label: &str,
    stoppable: bool,
    work: impl FnOnce(Report) -> anyhow::Result<T> + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let mut busy = ui.busy;
    let mut error = ui.error;
    if reject_busy(ui) {
        return;
    }
    busy.set(Some(Progress {
        done: 0,
        total: 0,
        step: label.to_string(),
    }));
    error.set(None);
    let mut done_line = ui.done;
    done_line.set(None);
    let mut can_stop = ui.stoppable;
    can_stop.set(stoppable);
    STOP.store(false, Ordering::Relaxed);
    let (sender, receiver) = async_channel::unbounded::<Progress>();
    let finished = Rc::new(Cell::new(false));
    let listening = Rc::clone(&finished);
    // Tasks often start from a dialog that closes at once; `spawn` would die with it.
    spawn_forever(async move {
        while let Ok(mut progress) = receiver.recv().await {
            // A report still queued when the work ends would mark the window busy for good.
            if listening.get() {
                break;
            }
            // Reports come faster than the window can draw; only the newest one matters.
            while let Ok(newer) = receiver.try_recv() {
                progress = newer;
            }
            busy.set(Some(progress));
        }
    });
    spawn_forever(async move {
        let result = blocking::unblock(move || {
            let report = |progress: Progress| {
                // The window may be gone; nothing to tell it then.
                let _ = sender.try_send(progress);
                !STOP.load(Ordering::Relaxed)
            };
            // A bug in the work shows as an error instead of ending the app.
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&report)))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("ufk stopped on an internal error")))
        })
        .await;
        finished.set(true);
        busy.set(None);
        can_stop.set(false);
        match result {
            Ok(value) => done(value),
            Err(failure) if failure.is::<progress::Stopped>() => {
                done_line.set(Some("Stopped".to_string()));
            }
            Err(failure) => {
                error.set(Some(format!("{failure:#}")));
                reload_view(ui);
            }
        }
    });
}

/// A failed task may have changed files before it stopped, so the shown state is read again.
fn reload_view(ui: Ui) {
    let Some(record) = ui.view.peek().as_ref().map(|game| game.record.clone()) else {
        return;
    };
    let mut view = ui.view;
    spawn_forever(async move {
        let id = record.id.clone();
        // The task's own error is already shown; a failed reload adds nothing to it.
        let Ok(fresh) = blocking::unblock(move || session::open_game(&record)).await else {
            return;
        };
        if view
            .peek()
            .as_ref()
            .is_some_and(|game| game.record.id == id)
        {
            view.set(Some(fresh));
        }
    });
}
