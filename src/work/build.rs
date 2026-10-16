use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use sha1::{Digest, Sha1};
use unicode_general_category::{GeneralCategory, get_general_category};

use crate::fonts::charsets::{self, Charset};
use crate::fonts::{extract, font_file};
use crate::game::folder::{self, Game};
use crate::game::scan::{self, LegacyFont, Ref, ScanResult, TmpFont};
use crate::game::{addressables, typetrees::Generator};
use crate::progress::{self, Report};
use crate::rebuild::inject::{self, Bake, Done, Options, Reset, Swap};
use crate::rebuild::{bake, recipe};
use crate::work::plan::{self, Entry, Kind, Place, Plan};
use crate::work::project::{self, Project};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub injected: Vec<Done>,
    pub not_baked: IndexMap<String, String>,
    pub catalogs: Vec<String>,
    /// Catalogs ufk cannot change, so the game may still check its bundles against them.
    pub unchanged: Vec<String>,
}

type Key<'a> = (Kind, &'a str, &'a str);
type FamilyKey = (Kind, String, String);
type Sources<'a> = HashMap<Key<'a>, PathBuf>;

pub fn select_entries(plan: &Plan) -> Result<Vec<&Entry>> {
    if plan.fonts.is_empty() {
        bail!("No fonts to fix, nothing to apply");
    }
    let chosen: Vec<&Entry> = plan
        .fonts
        .iter()
        .filter(|entry| !entry.skip && !entry.source.is_empty())
        .collect();
    if chosen.is_empty() {
        bail!("Nothing to apply: every font keeps its original. Choose a font for at least one");
    }
    Ok(chosen)
}

pub fn share_family_sources(plan: Plan) -> (Plan, Vec<String>) {
    let chosen = find_donors(&plan.fonts);
    let fonts: Vec<Entry> = plan
        .fonts
        .iter()
        .map(|entry| borrow_source(entry, &chosen))
        .collect();
    let notes = plan
        .fonts
        .iter()
        .zip(&fonts)
        .filter(|(old, new)| old.source != new.source)
        .map(|(_, new)| {
            let donor = chosen.get(&family_key(new)).map_or("", |d| d.font.as_str());
            format!(
                "{}: source is empty, using {} from {donor}, same font",
                new.font, new.source
            )
        })
        .collect();
    (Plan { fonts, ..plan }, notes)
}

pub fn family_key(entry: &Entry) -> FamilyKey {
    // Old TMP fonts store the family without its style, so the name tells Bold from Italic; sizes are digits.
    let name: String = plan::normalize(&entry.font)
        .chars()
        .filter(|ch| get_general_category(*ch) != GeneralCategory::DecimalNumber)
        .collect();
    (entry.kind, entry.family.clone(), name)
}

fn find_donors(fonts: &[Entry]) -> HashMap<FamilyKey, Entry> {
    let mut groups: IndexMap<FamilyKey, Vec<&Entry>> = IndexMap::new();
    for entry in fonts {
        if !entry.source.is_empty() && !entry.skip && !entry.family.is_empty() {
            groups.entry(family_key(entry)).or_default().push(entry);
        }
    }
    groups
        .into_iter()
        .filter_map(|(key, entries)| pick_donor(&entries).map(|donor| (key, donor.clone())))
        .collect()
}

fn pick_donor<'a>(entries: &[&'a Entry]) -> Option<&'a Entry> {
    // A font the translator chose outweighs one ufk suggested; two different choices are not guessed between.
    let by_hand: Vec<&Entry> = entries
        .iter()
        .copied()
        .filter(|entry| entry.source != entry.suggested_source)
        .collect();
    let chosen = if by_hand.is_empty() {
        entries.to_vec()
    } else {
        by_hand
    };
    let sources: HashSet<&str> = chosen.iter().map(|entry| entry.source.as_str()).collect();
    (sources.len() == 1).then(|| chosen[0])
}

fn borrow_source(entry: &Entry, chosen: &HashMap<FamilyKey, Entry>) -> Entry {
    let donor = chosen.get(&family_key(entry));
    match donor {
        Some(donor) if !entry.skip && entry.source.is_empty() => Entry {
            source: donor.source.clone(),
            ..entry.clone()
        },
        _ => entry.clone(),
    }
}

pub fn resolve_source(entry: &Entry, plan_path: &Path) -> Result<PathBuf> {
    let source = Path::new(&entry.source);
    let path = if source.is_absolute() {
        source.to_path_buf()
    } else {
        plan_path.parent().unwrap_or(Path::new("")).join(source)
    };
    if !path.is_file() {
        bail!(
            "{}: font file {} not found, choose another font",
            entry.font,
            entry.source
        );
    }
    Ok(path)
}

pub fn check_sources(
    entries: &[&Entry],
    plan_path: &Path,
    charset: &Charset,
) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    let mut seen = Vec::new();
    for entry in entries {
        let path = resolve_source(entry, plan_path).with_context(|| entry.font.clone())?;
        if seen.contains(&path) {
            continue;
        }
        let checked = std::fs::read(&path)
            .with_context(|| format!("Cannot read {}", path.display()))
            .and_then(|data| font_file::check(&data, charset))
            .with_context(|| entry.font.clone())?;
        lines.extend(describe_flaws(&file_name(&path), &checked, charset));
        seen.push(path);
    }
    Ok(lines)
}

pub fn describe_flaws(
    name: &str,
    checked: &font_file::FontCheck,
    charset: &Charset,
) -> Vec<String> {
    let empty = (!checked.empty.is_empty()).then(|| {
        format!(
            "{name}: {} have no outline and show as blank",
            charsets::format_chars(&checked.empty)
        )
    });
    let aliased = checked.aliased.chars().map(|ch| {
        let like = charset
            .lookalikes
            .iter()
            .find(|(letter, _)| *letter == ch)
            .map_or(String::new(), |(_, like)| like.to_string());
        format!("{name}: {ch} uses the glyph of {like}")
    });
    empty.into_iter().chain(aliased).collect()
}

pub fn recipe_path(work: &Project, entry: &Entry, place: &Place) -> PathBuf {
    // Windows paths stop at 260 characters, and bundle paths can be that long on their own.
    let digest = Sha1::digest(format!("{}|{}", place.file, place.cab).as_bytes());
    let file_tag: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    work.recipes.join(format!(
        "{} {file_tag} {}{}",
        extract::safe_stem(&entry.font),
        place.path_id,
        recipe::SUFFIX
    ))
}

pub fn scan_originals(
    files: &[String],
    work: &Project,
    data_dir: &Path,
    generator: Option<&Generator>,
    report: Report,
) -> HashMap<String, ScanResult> {
    progress::track(files, "Reading", String::clone, report)
        .map(|file| {
            let source = project::original_file(Some(&work.backup), data_dir, file);
            let home = data_dir
                .join(file)
                .parent()
                .unwrap_or(data_dir)
                .to_path_buf();
            let homes = HashMap::from([(source.clone(), home)]);
            (
                file.clone(),
                scan::scan_files(&[source], generator, "", &homes, &progress::silent),
            )
        })
        .collect()
}

fn is_at(reference: &Ref, name: &str, entry: &Entry, place: &Place) -> bool {
    name == entry.font && is_place(reference, place)
}

fn is_place(reference: &Ref, place: &Place) -> bool {
    reference.1 == place.path_id && (place.cab.is_empty() || reference.0 == place.cab)
}

fn scanned_file<'a>(
    scanned: &'a HashMap<String, ScanResult>,
    file: &str,
) -> Result<&'a ScanResult> {
    scanned.get(file).with_context(|| format!("'{file}'"))
}

pub fn find_game_font(
    entry: &Entry,
    place: &Place,
    scanned: &HashMap<String, ScanResult>,
    data_dir: &Path,
) -> Result<TmpFont> {
    let result = scanned_file(scanned, &place.file)?;
    let Some(font) = result
        .tmp
        .iter()
        .find(|font| is_at(&font.reference, &font.name, entry, place))
    else {
        let errors = if result.errors.is_empty() {
            String::new()
        } else {
            format!(" ({})", result.errors.join("; "))
        };
        bail!(
            "{}: not found in {}{errors}, scan the game again",
            entry.font,
            place.file
        );
    };
    Ok(TmpFont {
        file: data_dir.join(&place.file),
        ..font.clone()
    })
}

pub fn find_legacy_font<'a>(
    entry: &Entry,
    place: &Place,
    scanned: &'a HashMap<String, ScanResult>,
) -> Result<&'a LegacyFont> {
    scanned_file(scanned, &place.file)?
        .legacy
        .iter()
        .find(|font| is_at(&font.reference, &font.name, entry, place))
        .with_context(|| {
            format!(
                "{}: no Font in {}, scan the game again",
                entry.font, place.file
            )
        })
}

fn source_of<'a, 'e>(sources: &'a Sources<'e>, entry: &'e Entry) -> Result<&'a PathBuf> {
    sources
        .get(&plan::entry_key(entry))
        .with_context(|| format!("no source for {}", entry.font))
}

fn make_swaps(
    targets: &[(&Entry, &Place)],
    scanned: &HashMap<String, ScanResult>,
    sources: &Sources,
) -> Result<Vec<Swap>> {
    let fonts = targets
        .iter()
        .map(|(entry, place)| find_legacy_font(entry, place, scanned))
        .collect::<Result<Vec<_>>>()?;
    targets
        .iter()
        .zip(fonts)
        .map(|((entry, place), font)| {
            Ok(Swap {
                file: place.file.clone(),
                serialized_file: font.reference.0.clone(),
                path_id: place.path_id,
                font: entry.font.clone(),
                source: source_of(sources, entry)?.clone(),
            })
        })
        .collect()
}

fn find_dynamic_font<'a>(
    place: &Place,
    scanned: &'a HashMap<String, ScanResult>,
) -> Result<&'a TmpFont> {
    scanned_file(scanned, &place.file)?
        .tmp
        .iter()
        .find(|font| font.dynamic == Some(true) && is_place(&font.reference, place))
        .with_context(|| {
            format!(
                "No dynamic TMP font {} in {}, scan the game again",
                place.path_id, place.file
            )
        })
}

fn make_resets(
    entries: &[&Entry],
    scanned: &HashMap<String, ScanResult>,
    sources: &Sources,
    version: &str,
) -> Result<Vec<Reset>> {
    let places: Vec<(&Entry, &Place)> = entries
        .iter()
        .flat_map(|entry| entry.resets.iter().map(move |place| (*entry, place)))
        .collect();
    let fonts = places
        .iter()
        .map(|(_, place)| find_dynamic_font(place, scanned))
        .collect::<Result<Vec<_>>>()?;
    places
        .iter()
        .zip(fonts)
        .map(|((entry, place), font)| {
            Ok(Reset {
                file: place.file.clone(),
                serialized_file: font.reference.0.clone(),
                path_id: place.path_id,
                font: font.name.clone(),
                source: source_of(sources, entry)?.clone(),
                unity_version: version.to_string(),
            })
        })
        .collect()
}

fn list_lacking(
    entries: &[&Entry],
    sources: &Sources,
    charset: &Charset,
) -> Result<IndexMap<String, String>> {
    let mut lacking = IndexMap::new();
    for entry in entries {
        let path = source_of(sources, entry)?;
        let have: HashSet<u32> = std::fs::read(path)
            .with_context(|| format!("Cannot read {}", path.display()))
            .and_then(|data| font_file::load_cmap(&data))
            .with_context(|| entry.font.clone())?
            .into_keys()
            .collect();
        lacking.insert(
            entry.font.clone(),
            charsets::find_missing(&have, &charset.all()),
        );
    }
    lacking.retain(|_, chars: &mut String| !chars.is_empty());
    Ok(lacking)
}

fn write_recipe(path: PathBuf, font: &TmpFont, found: &Game, charset: &Charset) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create {}", parent.display()))?;
    }
    let built = recipe::build(font, &found.data_dir, charset, &found.unity_version)?;
    project::write_text(&path, &recipe::to_json(&built)?)?;
    Ok(path)
}

fn remove_tree(path: &Path) {
    // Whatever cannot be deleted shows up later, when the folder is written again.
    let _ = std::fs::remove_dir_all(path);
}

pub fn build(
    plan: &Plan,
    found: &Game,
    work: &Project,
    generator: Option<&Generator>,
    charset: &Charset,
    options: &Options,
    report: Report,
) -> Result<Built> {
    let context = Steps {
        found,
        work,
        generator,
        charset,
        options,
        report,
    };
    folder::reject_output_inside(&found.data_dir, &work.work)?;
    project::reject_changed_game(work, &found.data_dir)?;
    let entries = select_entries(plan)?;
    let sources = entries
        .iter()
        .map(|entry| Ok((plan::entry_key(entry), resolve_source(entry, &work.plan)?)))
        .collect::<Result<Sources>>()?;
    let legacy_entries: Vec<&Entry> = entries
        .iter()
        .copied()
        .filter(|entry| entry.kind == Kind::Legacy)
        .collect();
    // Read before anything is written, so a broken source font stops the build with nothing changed.
    let lacking = list_lacking(&legacy_entries, &sources, charset)?;
    let targets = list_places(&entries, Kind::Tmp);
    let swapped = list_places(&entries, Kind::Legacy);
    let files = list_changed_files(&targets, &swapped, &legacy_entries);
    let (catalogs, unchanged) = find_catalogs(work, &found.data_dir, &files)?;
    // Hashed before reading, so install notices any change made while build runs.
    let read_hashes = project::hash_originals(
        work,
        &found.data_dir,
        &[files.clone(), catalogs.clone()].concat(),
    )?;
    let scanned = scan_originals(&files, work, &found.data_dir, generator, report);
    let bakes = bake_targets(&context, &targets, &scanned, &sources)?;
    let mut not_baked = collect_not_baked(&targets, &bakes);
    let jobs = inject::Jobs {
        bakes,
        swaps: make_swaps(&swapped, &scanned, &sources)?,
        resets: make_resets(&legacy_entries, &scanned, &sources, &found.unity_version)?,
    };
    let injected = write_patched(&context, jobs, &catalogs)?;
    project::write_build_sources(work, &read_hashes)?;
    not_baked.extend(lacking);
    Ok(Built {
        injected,
        not_baked,
        catalogs,
        unchanged,
    })
}

struct Steps<'a> {
    found: &'a Game,
    work: &'a Project,
    generator: Option<&'a Generator>,
    charset: &'a Charset,
    options: &'a Options,
    report: Report<'a>,
}

fn list_changed_files(
    targets: &[(&Entry, &Place)],
    swapped: &[(&Entry, &Place)],
    legacy_entries: &[&Entry],
) -> Vec<String> {
    let cleared = legacy_entries.iter().flat_map(|entry| &entry.resets);
    targets
        .iter()
        .chain(swapped)
        .map(|(_, place)| place.file.clone())
        .chain(cleared.map(|place| place.file.clone()))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn bake_targets(
    context: &Steps,
    targets: &[(&Entry, &Place)],
    scanned: &HashMap<String, ScanResult>,
    sources: &Sources,
) -> Result<Vec<inject::Bake>> {
    let Steps {
        found,
        work,
        charset,
        report,
        ..
    } = *context;
    remove_tree(&work.recipes);
    let recipes = targets
        .iter()
        .map(|(entry, place)| {
            let path = recipe_path(work, entry, place);
            let font = find_game_font(entry, place, scanned, &found.data_dir)?;
            write_recipe(path, &font, found, charset)
        })
        .collect::<Result<Vec<_>>>()?;
    let jobs: Vec<(&Entry, &PathBuf)> = targets
        .iter()
        .map(|(entry, _)| *entry)
        .zip(&recipes)
        .collect();
    for (entry, path) in progress::track(&jobs, "Baking", |(entry, _)| entry.font.clone(), report) {
        bake::write(path, source_of(sources, entry)?).with_context(|| entry.font.clone())?;
    }
    recipes.iter().map(|path| inject::load_bake(path)).collect()
}

fn write_patched(context: &Steps, jobs: inject::Jobs, catalogs: &[String]) -> Result<Vec<Done>> {
    let Steps {
        found,
        work,
        generator,
        options,
        report,
        ..
    } = *context;
    let partial = with_suffix(&work.patched, ".partial");
    remove_tree(&partial);
    let files = inject::Files {
        data_dir: &found.data_dir,
        originals: Some(&work.backup),
        out_dir: &partial,
    };
    let injected = inject::inject_all(&files, jobs, generator, options, report)?;
    for catalog in catalogs {
        write_cleared_catalog(
            &project::original_file(Some(&work.backup), &found.data_dir, catalog),
            &partial.join(catalog),
        )?;
    }
    remove_tree(&work.patched);
    std::fs::rename(&partial, &work.patched).with_context(|| {
        format!(
            "Cannot rename {} to {}",
            partial.display(),
            work.patched.display()
        )
    })?;
    Ok(injected)
}

fn list_places<'a>(entries: &[&'a Entry], kind: Kind) -> Vec<(&'a Entry, &'a Place)> {
    entries
        .iter()
        .filter(|entry| entry.kind == kind)
        .flat_map(|entry| entry.places.iter().map(move |place| (*entry, place)))
        .collect()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

pub fn find_catalogs(
    work: &Project,
    data_dir: &Path,
    files: &[String],
) -> Result<(Vec<String>, Vec<String>)> {
    if !addressables::touches_bundles(files) {
        return Ok((Vec::new(), Vec::new()));
    }
    let original = |name: &str| project::original_file(Some(&work.backup), data_dir, name);
    let settings = original(addressables::SETTINGS);
    let found = if settings.is_file() {
        addressables::list_catalogs(&read_text(&settings)?)?
    } else {
        addressables::Catalogs {
            local: vec![
                addressables::JSON_CATALOG.to_string(),
                addressables::BINARY_CATALOG.to_string(),
            ],
            elsewhere: Vec::new(),
        }
    };
    let present: Vec<String> = found
        .local
        .into_iter()
        .filter(|name| original(name).is_file())
        .collect();
    let (readable, other): (Vec<String>, Vec<String>) = present
        .into_iter()
        .partition(|name| name.ends_with(".json") || name.ends_with(".bin"));
    Ok((readable, [other, found.elsewhere].concat()))
}

fn read_text(path: &Path) -> Result<String> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

pub fn write_cleared_catalog(source: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create {}", parent.display()))?;
    }
    if source.extension().is_some_and(|ext| ext == "bin") {
        let data =
            std::fs::read(source).with_context(|| format!("Cannot read {}", source.display()))?;
        let cleared = addressables::clear_binary_crcs(&data)?;
        return std::fs::write(target, cleared)
            .with_context(|| format!("Cannot write {}", target.display()));
    }
    project::write_text(target, &addressables::clear_crcs(&read_text(source)?)?)
}

fn collect_not_baked(targets: &[(&Entry, &Place)], bakes: &[Bake]) -> IndexMap<String, String> {
    let mut not_baked = IndexMap::new();
    for ((entry, _), done) in targets.iter().zip(bakes) {
        let added: HashSet<char> = done.recipe.added.chars().collect();
        let lacking: String = done
            .baked
            .get("not_baked")
            .and_then(|v| v.as_array())
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(|cp| cp.as_u64().and_then(|cp| u32::try_from(cp).ok()))
            .filter_map(char::from_u32)
            .filter(|ch| added.contains(ch))
            .collect();
        if !lacking.is_empty() {
            not_baked.insert(entry.font.clone(), lacking);
        }
    }
    not_baked
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(font: &str, family: &str, source: &str, places: &[Place]) -> Entry {
        Entry {
            font: font.to_string(),
            family: family.to_string(),
            missing: "Ґ".to_string(),
            source: source.to_string(),
            note: String::new(),
            places: places.to_vec(),
            skip: false,
            suggested_source: String::new(),
            suggested_skip: false,
            kind: Kind::Tmp,
            resets: Vec::new(),
            texts: 0,
        }
    }

    fn place(file: &str, path_id: i64, cab: &str) -> Place {
        Place {
            file: file.to_string(),
            path_id,
            cab: cab.to_string(),
        }
    }

    fn plan_of(fonts: Vec<Entry>) -> Plan {
        Plan {
            game: "game".to_string(),
            language: "uk".to_string(),
            chars: None,
            fonts,
            not_fixable: Vec::new(),
        }
    }

    fn sources(plan: &Plan) -> Vec<&str> {
        plan.fonts.iter().map(|e| e.source.as_str()).collect()
    }

    fn work() -> Project {
        project::open_project(Path::new("Game.ufk.json")).unwrap()
    }

    #[test]
    fn build_refuses_a_plan_without_any_source() {
        let body = Entry {
            note: "no matching font".to_string(),
            ..entry("Body SDF", "Body Regular", "", &[place("fonts", 1, "")])
        };
        let error = select_entries(&plan_of(vec![body])).unwrap_err();
        assert!(error.to_string().contains("Choose a font"));
    }

    #[test]
    fn an_empty_source_takes_the_font_chosen_for_another_size_of_it() {
        let places = [place("fonts", 1, "")];
        let small = entry("Roboto-14 SDF", "Roboto", "Roboto.ttf", &places);
        let large = entry("Roboto-24 SDF", "Roboto", "", &places);
        let bold = entry("Roboto-Bold SDF", "Roboto", "", &places);
        let legacy = Entry {
            kind: Kind::Legacy,
            ..entry("Roboto-14", "Roboto", "", &places)
        };
        let (shared, notes) = share_family_sources(plan_of(vec![small, large, bold, legacy]));
        assert_eq!(sources(&shared), ["Roboto.ttf", "Roboto.ttf", "", ""]);
        assert_eq!(
            notes,
            ["Roboto-24 SDF: source is empty, using Roboto.ttf from Roboto-14 SDF, same font"]
        );
    }

    #[test]
    fn a_source_chosen_by_hand_beats_a_suggested_one_and_two_choices_are_not_guessed() {
        let places = [place("fonts", 1, "")];
        let empty = entry("Body 10 SDF", "Body", "", &places);
        let suggested = Entry {
            suggested_source: "game.ttf".to_string(),
            ..entry("Body 12 SDF", "Body", "game.ttf", &places)
        };
        let chosen = entry("Body 14 SDF", "Body", "Roboto.ttf", &places);
        let other = entry("Body 16 SDF", "Body", "Other.ttf", &places);
        let (one, _) =
            share_family_sources(plan_of(vec![suggested, chosen.clone(), empty.clone()]));
        let (two, _) = share_family_sources(plan_of(vec![chosen, other, empty]));
        assert_eq!(
            (one.fonts[2].source.as_str(), two.fonts[2].source.as_str()),
            ("Roboto.ttf", "")
        );
    }

    #[test]
    fn copies_in_different_files_get_their_own_recipe() {
        let body = entry(
            "Body SDF",
            "Body",
            "my.ttf",
            &[place("fonts", 7, ""), place("ui", 7, "")],
        );
        assert_ne!(
            recipe_path(&work(), &body, &body.places[0]),
            recipe_path(&work(), &body, &body.places[1])
        );
    }

    #[test]
    fn recipes_stay_flat_for_deep_bundle_paths() {
        let deep = "StreamingAssets/aa/StandaloneWindows64/scenes/managers/gameplay.unity_9044473112c6dbd5.bundle";
        let body = entry("Body SDF", "Body", "my.ttf", &[place(deep, 7, "")]);
        let path = recipe_path(&work(), &body, &body.places[0]);
        assert_eq!(path.parent(), Some(work().recipes.as_path()));
    }

    #[test]
    fn a_place_picks_its_own_serialized_file_inside_a_bundle() {
        let font = |cab: &str| LegacyFont {
            file: PathBuf::from("fonts"),
            reference: (cab.to_string(), 7),
            name: "Body".to_string(),
            data: b"x".to_vec(),
            os_names: Vec::new(),
            dynamic: false,
        };
        let result = ScanResult {
            legacy: vec![font("CAB-a"), font("CAB-b")],
            ..ScanResult::default()
        };
        let scanned = HashMap::from([("fonts".to_string(), result)]);
        let body = Entry {
            kind: Kind::Legacy,
            ..entry("Body", "Body", "my.ttf", &[])
        };
        let places = [place("fonts", 7, "CAB-a"), place("fonts", 7, "CAB-b")];
        let found: Vec<&str> = places
            .iter()
            .map(|p| {
                find_legacy_font(&body, p, &scanned)
                    .unwrap()
                    .reference
                    .0
                    .as_str()
            })
            .collect();
        assert_eq!(found, ["CAB-a", "CAB-b"]);
    }
}
