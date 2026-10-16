use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub step: String,
}

/// Called from the thread that does the work; false once the user stopped it.
pub type Report<'a> = &'a (dyn Fn(Progress) -> bool + Sync);

#[derive(Debug)]
pub struct Stopped;

impl fmt::Display for Stopped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Stopped")
    }
}

impl std::error::Error for Stopped {}

pub fn silent(_: Progress) -> bool {
    true
}

pub fn check(report: Report, step: &str) -> anyhow::Result<()> {
    let going = report(Progress {
        done: 0,
        total: 0,
        step: step.to_string(),
    });
    going.then_some(()).ok_or_else(|| Stopped.into())
}

pub fn track<'a, T>(
    items: &'a [T],
    step: &'a str,
    name: impl Fn(&T) -> String + 'a,
    report: Report<'a>,
) -> impl Iterator<Item = &'a T> + 'a {
    let total = items.len();
    let stopped = Rc::new(Cell::new(false));
    let seen = Rc::clone(&stopped);
    let shown = items.iter().enumerate().map_while(move |(done, item)| {
        let going = report(Progress {
            done,
            total,
            step: format!("{step} {}", name(item)),
        });
        stopped.set(!going);
        going.then_some(item)
    });
    let finished = std::iter::from_fn(move || {
        if !seen.get() {
            report(Progress {
                done: total,
                total,
                step: step.to_string(),
            });
        }
        None
    });
    shown.chain(finished)
}
