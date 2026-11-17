use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::prefix::Roots;
use crate::scan::{self, Filter, GameSaves, SaveFile};
use crate::ui::{Progress, ProgressMode, tilde};

const INDEX_FILE: &str = "noisette.toml";
const FILES_DIR: &str = "files";
const BACKUP_SUFFIX: &str = ".noisette-bak";
const STAGING_SUFFIX: &str = ".partial";
const FORMAT_VERSION: u32 = 1;
const UNSAFE_NAME_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

#[derive(Serialize, Deserialize)]
struct Index<G> {
    version: u32,
    #[serde(flatten)]
    saves: G,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Placement {
    Created,
    Replaced,
    Overwritten,
    Unchanged,
}

impl Placement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Replaced => "replaced",
            Self::Overwritten => "overwritten",
            Self::Unchanged => "unchanged",
        }
    }
}

pub struct ImportOptions<'a> {
    pub force: bool,
    pub dry_run: bool,
    pub store_user_id: Option<&'a str>,
    pub progress: ProgressMode,
}

pub fn export(games: &[GameSaves], out: &Path, mode: ProgressMode) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("creating {}", tilde(out)))?;
    let mut progress = Progress::new(mode, games.iter().map(GameSaves::total_size).sum());
    for game in games {
        progress.start(&game.name);
        export_game(game, &out.join(folder_name(&game.name)), &mut progress)
            .with_context(|| format!("exporting {}", game.name))?;
    }
    Ok(())
}

fn export_game(game: &GameSaves, dest: &Path, progress: &mut Progress) -> Result<()> {
    if dest.exists() && !dest.join(INDEX_FILE).is_file() {
        bail!("{} exists and isn't a noisette backup", tilde(dest));
    }
    let staging = with_suffix(dest, STAGING_SUFFIX);
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    for file in &game.files {
        copy_preserving(
            &file.source,
            &staging.join(FILES_DIR).join(stored_path(file)),
        )?;
        progress.advance(file.size);
    }
    let index = Index {
        version: FORMAT_VERSION,
        saves: game,
    };
    let index_path = staging.join(INDEX_FILE);
    fs::write(&index_path, toml::to_string_pretty(&index)?)?;
    sync_path(&index_path)?;
    sync_tree(&staging)?;
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::rename(&staging, dest)?;
    if let Some(parent) = dest.parent() {
        sync_path(parent)?;
    }
    Ok(())
}

pub fn load(from: &Path, filter: &Filter) -> Result<Vec<GameSaves>> {
    let dirs = if from.join(INDEX_FILE).is_file() {
        vec![from.to_owned()]
    } else {
        backup_dirs(from)?
    };
    if dirs.is_empty() {
        bail!("no noisette backups in {}", tilde(from));
    }
    let mut games = Vec::new();
    for dir in dirs {
        let mut game = read_backup(&dir).with_context(|| format!("reading {}", tilde(&dir)))?;
        if filter.wants_game(|query| game.is_called(query)) {
            game.files.retain(|file| filter.keeps(file));
            games.push(game);
        }
    }
    games.retain(|game| !game.files.is_empty());
    Ok(games)
}

fn backup_dirs(from: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(from).with_context(|| format!("reading {}", tilde(from)))?;
    let mut dirs: Vec<_> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|dir| dir.join(INDEX_FILE).is_file())
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn read_backup(dir: &Path) -> Result<GameSaves> {
    let index: Index<GameSaves> = toml::from_str(&fs::read_to_string(dir.join(INDEX_FILE))?)?;
    if index.version != FORMAT_VERSION {
        bail!("unsupported backup version {}", index.version);
    }
    let mut game = index.saves;
    for file in &mut game.files {
        file.source = dir.join(FILES_DIR).join(stored_path(file));
    }
    Ok(game)
}

pub fn import(
    games: &[GameSaves],
    roots: &Roots,
    options: &ImportOptions,
) -> Result<BTreeMap<Placement, usize>> {
    let mut placed = BTreeMap::new();
    let mut touched_dirs = BTreeSet::new();
    let mut progress = Progress::new(
        options.progress,
        games.iter().map(GameSaves::total_size).sum(),
    );
    for game in games {
        progress.start(&game.name);
        for file in &game.files {
            let Some(root) = roots.resolve(file.root) else {
                bail!(
                    "{} needs {}",
                    game.name,
                    file.root.flag().unwrap_or(file.root.as_str())
                );
            };
            let target = root.join(file.rel_for(options.store_user_id));
            let placement = placement_of(file, &target, options.force);
            if !options.dry_run && placement != Placement::Unchanged {
                place(file, &target, placement)
                    .with_context(|| format!("importing {}", tilde(&target)))?;
                touched_dirs.extend(target.parent().map(Path::to_path_buf));
            }
            *placed.entry(placement).or_default() += 1;
            progress.advance(file.size);
        }
    }
    for dir in &touched_dirs {
        sync_path(dir)?;
    }
    Ok(placed)
}

fn placement_of(file: &SaveFile, target: &Path, force: bool) -> Placement {
    match fs::metadata(target) {
        Err(_) => Placement::Created,
        Ok(meta) if meta.len() == file.size && scan::unix_mtime(&meta) == file.modified => {
            Placement::Unchanged
        }
        Ok(_) if force => Placement::Overwritten,
        Ok(_) => Placement::Replaced,
    }
}

fn place(file: &SaveFile, target: &Path, placement: Placement) -> Result<()> {
    match placement {
        Placement::Unchanged => return Ok(()),
        Placement::Replaced => fs::rename(target, with_suffix(target, BACKUP_SUFFIX))?,
        Placement::Created | Placement::Overwritten => {}
    }
    copy_preserving(&file.source, target)
}

fn copy_preserving(source: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, target).with_context(|| format!("copying {}", tilde(source)))?;
    let modified = fs::metadata(source)?.modified()?;
    fs::set_times(target, FileTimes::new().set_modified(modified))?;
    sync_path(target)
}

fn sync_path(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|handle| handle.sync_all())
        .with_context(|| format!("syncing {}", tilde(path)))
}

fn sync_tree(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path())?;
        }
    }
    sync_path(dir)
}

fn stored_path(file: &SaveFile) -> PathBuf {
    Path::new(file.root.as_str()).join(file.rel_for(None))
}

fn folder_name(game: &str) -> String {
    let name: String = game
        .chars()
        .map(|c| {
            if UNSAFE_NAME_CHARS.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    name.trim_end_matches(['.', ' ']).to_owned()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}
