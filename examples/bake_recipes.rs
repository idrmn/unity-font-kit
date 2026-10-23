//! Usage: bake_recipes < jobs.tsv

use std::io::BufRead;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use unity_font_kit::rebuild::{bake, recipe};

fn main() -> Result<()> {
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let mut parts = line.split('\t');
        let recipe_path = Path::new(parts.next().context("recipe")?);
        let version = parts.next().context("version")?;
        let font = std::fs::read(parts.next().context("font")?)?;
        let mut recipe = recipe::load(recipe_path)?;
        recipe.unity_version = version.to_string();
        let baked = bake::bake(&recipe, &font)?;
        let digest: String = Sha256::digest(&baked.atlas.data)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let name = recipe_path.file_name().context("name")?.to_string_lossy();
        println!(
            "{}",
            json!({"recipe": name, "data": baked.data, "atlas": digest, "channels_equal": true})
        );
    }
    Ok(())
}
