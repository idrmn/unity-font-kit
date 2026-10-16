use std::cell::Cell;
use std::rc::Rc;

use freya::prelude::{WritableUtils, spawn};

use super::Ui;
use crate::session::Progress;

pub fn run<T: Send + 'static>(
    ui: Ui,
    label: &str,
    work: impl FnOnce(&(dyn Fn(Progress) + Sync)) -> anyhow::Result<T> + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let mut busy = ui.busy;
    let mut error = ui.error;
    if busy.read().is_some() {
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
    let (sender, receiver) = async_channel::unbounded::<Progress>();
    let finished = Rc::new(Cell::new(false));
    let listening = Rc::clone(&finished);
    spawn(async move {
        while let Ok(progress) = receiver.recv().await {
            // A report still queued when the work ends would mark the window busy for good.
            if listening.get() {
                break;
            }
            busy.set(Some(progress));
        }
    });
    spawn(async move {
        let result = blocking::unblock(move || {
            let report = |progress: Progress| {
                // The window may be gone; nothing to tell it then.
                let _ = sender.try_send(progress);
            };
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&report)))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("ufk stopped on an internal error")))
        })
        .await;
        finished.set(true);
        busy.set(None);
        match result {
            Ok(value) => done(value),
            Err(failure) => error.set(Some(format!("{failure:#}"))),
        }
    });
}
