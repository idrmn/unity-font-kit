use anyhow::{Context, Result};
use serde_json::Value;

use crate::game::assets::{self, Tpk};
use crate::game::folder;
use crate::game::scan::{self, Pointers, Reader};
use crate::game::texture;
use crate::game::typetrees;
use crate::rebuild::render::Gray;
use crate::work::plan::{Entry, Kind, Place};
use crate::work::project;

use super::state::open_work;
use super::{FontKey, GameView};

pub fn atlas_png(view: &GameView, key: &FontKey) -> Result<Option<Vec<u8>>> {
    match read_game_font(view, key)? {
        Some((_, atlas)) => Ok(Some(encode_gray(&atlas)?)),
        None => Ok(None),
    }
}

pub(super) fn find_entry<'a>(view: &'a GameView, key: &FontKey) -> Option<&'a Entry> {
    view.plan
        .as_ref()?
        .fonts
        .iter()
        .find(|entry| FontKey::of(entry) == *key)
}

pub(super) fn read_legacy_data(view: &GameView, entry: &Entry) -> Result<Option<Vec<u8>>> {
    let Some(place) = entry.places.first() else {
        return Ok(None);
    };
    let game = folder::locate(&view.record.folder)?;
    let work = open_work(&view.home, &view.record)?;
    let path = project::original_file(Some(&work.backup), &game.data_dir, &place.file);
    let home = game.data_dir.join(&place.file);
    let source = assets::open(&path, home.parent())?;
    let found = source.units.iter().find_map(|unit| {
        let named = place.cab.is_empty() || unit.name == place.cab;
        Some((
            unit,
            unit.file.get_object_info(place.path_id).filter(|_| named)?,
        ))
    });
    let Some((unit, info)) = found else {
        return Ok(None);
    };
    let tpk = Tpk::embedded();
    let node = assets::node_for(unit, info, &tpk).context("No Font typetree")?;
    let font = assets::read_value(unit, info, &node, false)?;
    let data = assets::read_bytes(font.get("m_FontData"));
    Ok((!data.is_empty()).then_some(data))
}

fn read_game_font(
    view: &GameView,
    key: &FontKey,
) -> Result<Option<(serde_json::Map<String, Value>, Gray)>> {
    let Some((entry, place)) = find_tmp_place(view, key) else {
        return Ok(None);
    };
    let game = folder::locate(&view.record.folder)?;
    let work = open_work(&view.home, &view.record)?;
    let path = project::original_file(Some(&work.backup), &game.data_dir, &place.file);
    let home = game.data_dir.join(&place.file);
    let source = assets::open(&path, home.parent())?;
    let generator =
        typetrees::build_generator(&game.unity_version, typetrees::find_dll_dir(&game)?)?;
    let reader = Reader::new(generator.as_ref(), "");
    let found = source.units.iter().find_map(|unit| {
        let named = place.cab.is_empty() || unit.name == place.cab;
        Some((
            unit,
            unit.file.get_object_info(place.path_id).filter(|_| named)?,
        ))
    });
    let (unit, info) =
        found.with_context(|| format!("{}: not found in {}", entry.font, place.file))?;
    let node = scan::tree_node(&source, unit, info, &reader)
        .with_context(|| format!("{}: cannot read the font", entry.font))?;
    let Value::Object(tree) = assets::read_value(unit, info, &node, true)? else {
        return Ok(None);
    };
    let font = scan::read_tmp_font(&path, &Pointers::of(unit), place.path_id, &tree)?;
    let tree = tree.clone();
    let Some((file, path_id)) = font.atlases.iter().flatten().flatten().next() else {
        return Ok(None);
    };
    let Some(atlas_unit) = source.units.iter().find(|unit| &unit.name == file) else {
        return Ok(None);
    };
    let atlas = atlas_unit
        .file
        .get_object_info(*path_id)
        .context("No atlas texture")?;
    let tpk = Tpk::embedded();
    let node = assets::node_for(atlas_unit, atlas, &tpk).context("No Texture2D typetree")?;
    let texture = assets::read_value(atlas_unit, atlas, &node, false)?;
    Ok(Some((tree, texture::read_alpha(&texture, &source)?)))
}

fn encode_gray(image: &Gray) -> Result<Vec<u8>> {
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(
        &mut png,
        u32::try_from(image.width)?,
        u32::try_from(image.height)?,
    );
    encoder.set_color(png::ColorType::Grayscale);
    encoder.write_header()?.write_image_data(&image.data)?;
    Ok(png)
}
fn find_tmp_place<'a>(view: &'a GameView, key: &FontKey) -> Option<(&'a Entry, &'a Place)> {
    let entry = view
        .plan
        .as_ref()?
        .fonts
        .iter()
        .find(|entry| FontKey::of(entry) == *key && entry.kind == Kind::Tmp)?;
    Some((entry, entry.places.first()?))
}
