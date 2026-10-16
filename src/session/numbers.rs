//! Built in its own project folder, so a second Find in game after Restore only copies the files again.

use anyhow::{Context, Result, bail};

use crate::fonts::numbers;
use crate::game::folder::Game;
use crate::game::typetrees;
use crate::progress::{Progress, Report};
use crate::rebuild::inject::Options;
use crate::work::build;
use crate::work::plan::{self, Entry, Plan};
use crate::work::project::{self, Project};

use super::apply::{prepare_change, reject_unwritable};
use super::export::{STAMP, is_fresh, open_side_project, select_fonts};
use super::scanning::prepare_generator;
use super::state::{self, Numbered, Saved, detect_state, open_game_in};
use super::{GameState, GameView};

const NUMBERS_DIR: &str = "numbers";

/// Every font draws its row number, so one game start shows which row each text uses.
pub fn show_numbers(view: &GameView, progress: Report) -> Result<GameView> {
    let (game, work) = prepare_change(view)?;
    let Some(plan) = view.plan.as_ref() else {
        bail!("Scan the game first");
    };
    // On top of applied fonts, Restore would bring back the originals and drop them unseen.
    if detect_state(&work, &game.data_dir)? != GameState::Original {
        bail!("Restore the original fonts first");
    }
    reject_unwritable(&game.data_dir)?;
    let side = open_side_project(&work, NUMBERS_DIR)?;
    let numbered = number_fonts(plan, &work, &side)?;
    build_once(&numbered, &game, &side, progress)?;
    progress(Progress {
        done: 0,
        total: 1,
        step: "Copying the new files into the game".to_string(),
    });
    project::install(&side, &game.data_dir)?;
    let saved = state::load_saved(&work)?;
    state::store_saved(
        &work,
        &Saved {
            applied: Vec::new(),
            results: Vec::new(),
            numbers: list_numbers(plan),
            ..saved
        },
    )?;
    open_game_in(&view.home, &view.record)
}

fn build_once(numbered: &Plan, game: &Game, side: &Project, progress: Report) -> Result<()> {
    let stamp = format!(
        "{}
{}",
        env!("CARGO_PKG_VERSION"),
        plan::to_json(numbered)?
    );
    if is_fresh(side, &game.data_dir, &stamp)? {
        return Ok(());
    }
    project::write_text(&side.plan, &plan::to_json(numbered)?)?;
    let charset = plan::read_charset(numbered, &side.plan)?;
    let generator = prepare_generator(game, progress)?;
    let options = Options {
        raw_note: typetrees::describe_raw_reading(game),
    };
    build::build(
        numbered,
        game,
        side,
        generator.as_ref(),
        &charset,
        &options,
        progress,
    )?;
    project::write_text(&side.work.join(STAMP), &stamp)
}

fn list_numbers(plan: &Plan) -> Vec<Numbered> {
    plan.fonts
        .iter()
        .enumerate()
        .map(|(index, entry)| Numbered {
            kind: entry.kind,
            font: entry.font.clone(),
            family: entry.family.clone(),
            number: index + 1,
        })
        .collect()
}

/// Every font gets its own number font, with paths the side project can read.
fn number_fonts(plan: &Plan, work: &Project, side: &Project) -> Result<Plan> {
    let folder = side.work.join("fonts");
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("Cannot create {}", folder.display()))?;
    let fonts = plan
        .fonts
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let path = folder.join(format!("{}.ttf", index + 1));
            std::fs::write(&path, numbers::make_number_font(index + 1)?)
                .with_context(|| format!("Cannot write {}", path.display()))?;
            Ok(Entry {
                source: path.display().to_string(),
                skip: false,
                ..entry.clone()
            })
        })
        .collect::<Result<Vec<Entry>>>()?;
    let every: Vec<_> = fonts.iter().map(super::FontKey::of).collect();
    let chosen = Plan {
        fonts,
        ..plan.clone()
    };
    Ok(select_fonts(&chosen, &work.plan, &every))
}
