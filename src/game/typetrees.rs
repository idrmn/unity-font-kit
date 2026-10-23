use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rabex::UnityVersion;
use rabex::objects::ClassId;
use rabex::tpk::TpkTypeTreeBlob;
use rabex::typetree::TypeTreeNode;
use unity_typetree_gen::AssemblyTypeTreeGenerator;

use crate::game::folder::{Backend, Game};
use crate::game::il2cpp;

pub struct Generator {
    assemblies: AssemblyTypeTreeGenerator,
    dll_dir: PathBuf,
    version: UnityVersion,
}

impl fmt::Debug for Generator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Generator").finish_non_exhaustive()
    }
}

pub fn find_dll_dir(game: &Game) -> Result<Option<PathBuf>> {
    let dir = match game.backend {
        Backend::Mono => game.managed_dir(),
        Backend::Il2cpp => il2cpp::find_dll_dir(game)?,
    };
    Ok(has_dlls(&dir).then_some(dir))
}

pub fn build_generator(unity_version: &str, dll_dir: Option<PathBuf>) -> Result<Option<Generator>> {
    let Some(dll_dir) = dll_dir else {
        return Ok(None);
    };
    let version: UnityVersion = unity_version.parse()?;
    Ok(Some(Generator {
        assemblies: AssemblyTypeTreeGenerator::new(version.clone()),
        dll_dir,
        version,
    }))
}

pub fn generate_script_tree(
    generator: &Generator,
    assembly: &str,
    namespace: &str,
    class: &str,
) -> Result<TypeTreeNode> {
    let file_name = assembly_file_name(assembly);
    let script = generator
        .assemblies
        .generate_from_dir(&generator.dll_dir, &file_name, namespace, class)?
        .with_context(|| format!("no {namespace}.{class} in {assembly}"))?;
    let base = TpkTypeTreeBlob::embedded()
        .get_typetree_node(ClassId::MonoBehaviour, &generator.version)
        .with_context(|| format!("no MonoBehaviour typetree for Unity {}", generator.version))?;
    Ok(prepend_children(script, &base))
}

#[must_use]
pub fn describe_raw_reading(game: &Game) -> String {
    let source = match game.backend {
        Backend::Mono => "DLLs",
        Backend::Il2cpp => "code",
    };
    format!("no typetree in the game's {source}")
}

#[must_use]
pub fn has_dlls(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let name = if cfg!(windows) {
                name.to_lowercase()
            } else {
                name
            };
            name.ends_with(".dll")
        })
    })
}

/// Unity 6 MonoScripts name the assembly without `.dll`; older versions include it.
fn assembly_file_name(assembly: &str) -> String {
    if assembly.ends_with(".dll") {
        return assembly.to_string();
    }
    format!("{assembly}.dll")
}

fn prepend_children(mut tree: TypeTreeNode, base: &TypeTreeNode) -> TypeTreeNode {
    tree.children = base.children.iter().cloned().chain(tree.children).collect();
    tree
}
