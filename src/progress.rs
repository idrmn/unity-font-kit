#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub step: String,
}

/// Called from the thread that does the work.
pub type Report<'a> = &'a (dyn Fn(Progress) + Sync);

pub fn silent(_: Progress) {}

pub fn track<'a, T>(
    items: &'a [T],
    step: &'a str,
    name: impl Fn(&T) -> String + 'a,
    report: Report<'a>,
) -> impl Iterator<Item = &'a T> + 'a {
    let total = items.len();
    let shown = items.iter().enumerate().map(move |(done, item)| {
        report(Progress {
            done,
            total,
            step: format!("{step} {}", name(item)),
        });
        item
    });
    let finished = std::iter::from_fn(move || {
        report(Progress {
            done: total,
            total,
            step: step.to_string(),
        });
        None
    });
    shown.chain(finished)
}
