//! Render the GUI headless into PNG files, to look at screens without a display.
//! Usage: UFK_DATA=DIR gui_snapshots OUT_DIR GAME_COPY FONT [SCALE]
//! Prepares the data folder through the session layer first (add, quick scan, every font replaced, apply);
//! GAME_COPY is written to, so pass a copy.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use freya::prelude::Size2D;
use freya_testing::prelude::TestingRunner;
use unity_font_kit::{progress, session};

fn settle(runner: &mut TestingRunner, millis: u64) {
    runner.poll(Duration::from_millis(20), Duration::from_millis(millis));
}

fn shoot(
    runner: &mut TestingRunner,
    (out, scale): (&Path, f64),
    name: &str,
    click: Option<(f64, f64)>,
) {
    if let Some((x, y)) = click {
        let point = (x * scale, y * scale);
        runner.move_cursor(point);
        runner.click_cursor(point);
        settle(runner, 3000);
    }
    runner.render_to_file(out.join(name));
}

fn prepare(game: &Path, font: &Path) -> Result<()> {
    let (library, record) = session::add_game(&session::load_library()?, game)?;
    session::save_library(&library)?;
    let letters = session::letters_from_preset("uk")?;
    let mut view = session::scan(&record, false, &letters, &progress::silent)?;
    let keys: Vec<session::FontKey> = view
        .plan
        .iter()
        .flat_map(|plan| plan.fonts.iter())
        .filter(|entry| !entry.skip)
        .map(session::FontKey::of)
        .collect();
    for key in &keys {
        view = session::set_source(&view, key, Some(font.to_path_buf()))?;
    }
    session::apply(&view, &progress::silent)?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out = Path::new(&args[1]);
    prepare(Path::new(&args[2]), Path::new(&args[3]))?;
    let scale: f64 = args
        .get(4)
        .and_then(|scale| scale.parse().ok())
        .unwrap_or(1.0);
    let (mut runner, _) = TestingRunner::new(
        unity_font_kit::gui::app,
        Size2D::new(1180. * scale as f32, 700. * scale as f32),
        |_| {},
        scale,
    );
    settle(&mut runner, 600);
    shoot(&mut runner, (out, scale), "1-empty.png", None);
    shoot(&mut runner, (out, scale), "2-game.png", Some((120., 60.)));
    shoot(
        &mut runner,
        (out, scale),
        "3-legacy.png",
        Some((450., 251.)),
    );
    shoot(&mut runner, (out, scale), "4-tmp.png", Some((450., 304.)));
    shoot(
        &mut runner,
        (out, scale),
        "5-picker.png",
        Some((1018., 207.)),
    );
    shoot(&mut runner, (out, scale), "6-cancel.png", Some((10., 10.)));
    shoot(&mut runner, (out, scale), "7-remove.png", Some((253., 73.)));
    shoot(&mut runner, (out, scale), "8-cancel.png", Some((10., 10.)));
    shoot(&mut runner, (out, scale), "9-select.png", Some((370., 94.)));
    Ok(())
}
