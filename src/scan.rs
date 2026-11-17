use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use clap::ValueEnum;
use glob::{MatchOptions, Pattern};
use serde::{Deserialize, Serialize};

use crate::manifest::{self, Manifest, Tag};
use crate::prefix::Roots;
use crate::template::{Root, STORE_USER_ID, Segment, Template};

const CASELESS: MatchOptions = MatchOptions {
    case_sensitive: false,
    require_literal_separator: false,
    require_literal_leading_dot: false,
};

pub struct Filter<'a> {
    pub games: &'a [String],
    pub tags: &'a [Tag],
    pub exclude: &'a [Pattern],
    pub store_user_id: Option<&'a str>,
}

impl Filter<'_> {
    pub fn wants_game(&self, is_called: impl Fn(&str) -> bool) -> bool {
        self.games.is_empty() || self.games.iter().any(|query| is_called(query))
    }

    pub fn keeps(&self, file: &SaveFile) -> bool {
        file.tags.iter().any(|tag| self.tags.contains(tag)) && !self.excludes(&file.rel)
    }

    fn excludes(&self, rel: &Path) -> bool {
        self.exclude.iter().any(|pattern| pattern.matches_path(rel))
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Sort {
    Name,
    Size,
    Recent,
}

impl Sort {
    pub fn apply(self, games: &mut [GameSaves]) {
        match self {
            Self::Name => {}
            Self::Size => games.sort_by_key(|game| Reverse(game.total_size())),
            Self::Recent => games.sort_by_key(|game| Reverse(game.last_modified())),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct GameSaves {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub store_ids: Vec<u64>,
    pub files: Vec<SaveFile>,
}

impl GameSaves {
    pub fn is_called(&self, query: &str) -> bool {
        manifest::matches_game(&self.name, self.store_ids.iter().copied(), query)
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    pub fn last_modified(&self) -> u64 {
        self.files
            .iter()
            .map(|file| file.modified)
            .max()
            .unwrap_or_default()
    }

    pub fn store_user_ids(&self) -> BTreeSet<&str> {
        self.files
            .iter()
            .filter_map(|file| file.store_user_id.as_deref())
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
pub struct SaveFile {
    pub root: Root,
    pub rel: PathBuf,
    pub tags: Vec<Tag>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store_user_id: Option<String>,
    pub size: u64,
    pub modified: u64,
    #[serde(skip)]
    pub source: PathBuf,
}

impl SaveFile {
    pub fn rel_for(&self, store_user_id: Option<&str>) -> PathBuf {
        let id = store_user_id.or(self.store_user_id.as_deref());
        self.rel
            .iter()
            .map(|part| match id {
                Some(id) if part == STORE_USER_ID => id.as_ref(),
                _ => part,
            })
            .collect()
    }
}

#[derive(Clone)]
struct Cursor {
    path: PathBuf,
    rel: PathBuf,
    store_user_id: Option<String>,
}

impl Cursor {
    fn enter(&self, name: &str, rel_name: &str) -> Self {
        Self {
            path: self.path.join(name),
            rel: self.rel.join(rel_name),
            store_user_id: self.store_user_id.clone(),
        }
    }
}

pub fn scan(manifest: &Manifest, roots: &Roots, filter: &Filter) -> Vec<GameSaves> {
    manifest
        .iter()
        .filter(|(name, game)| filter.wants_game(|query| game.is_called(name, query)))
        .filter_map(|(name, game)| {
            let mut found = BTreeMap::new();
            let os = roots.os();
            for (raw, rule) in game.files.iter().filter(|(_, rule)| rule.applies_to(os)) {
                let tags = rule.tags_within(filter.tags);
                if tags.is_empty() {
                    continue;
                }
                for template in Template::expand(raw, os) {
                    let walker = Walker {
                        roots,
                        filter,
                        root: template.root,
                        tags: &tags,
                        found: &mut found,
                    };
                    walker.start(&template.segments);
                }
            }
            (!found.is_empty()).then(|| GameSaves {
                name: name.clone(),
                store_ids: game.store_ids().collect(),
                files: found.into_values().collect(),
            })
        })
        .collect()
}

struct Walker<'a> {
    roots: &'a Roots,
    filter: &'a Filter<'a>,
    root: Root,
    tags: &'a [Tag],
    found: &'a mut BTreeMap<PathBuf, SaveFile>,
}

impl Walker<'_> {
    fn start(mut self, segments: &[Segment]) {
        if let Some(path) = self.roots.resolve(self.root) {
            self.walk(
                Cursor {
                    path,
                    rel: PathBuf::new(),
                    store_user_id: None,
                },
                segments,
            );
        }
    }

    fn walk(&mut self, at: Cursor, segments: &[Segment]) {
        let Some((segment, rest)) = segments.split_first() else {
            return self.collect(at);
        };
        match segment {
            Segment::Literal(name) => self.enter_named(&at, name, rest),
            Segment::OsUserName => {
                if let Some(user) = self.roots.wine_user() {
                    self.enter_named(&at, user, rest);
                }
            }
            Segment::Pattern(pattern) => {
                for name in children(&at.path).filter(|name| pattern.matches_with(name, CASELESS)) {
                    self.walk(at.enter(&name, &name), rest);
                }
            }
            Segment::StoreUserId => match self.filter.store_user_id {
                Some(id) => self.enter_store_user(&at, id, rest),
                None => {
                    for id in children(&at.path).filter(|name| at.path.join(name).is_dir()) {
                        self.enter_store_user(&at, &id, rest);
                    }
                }
            },
            Segment::AnyDepth => {
                self.walk(at.clone(), rest);
                for name in children(&at.path).filter(|name| at.path.join(name).is_dir()) {
                    self.walk(at.enter(&name, &name), segments);
                }
            }
        }
    }

    fn enter_named(&mut self, at: &Cursor, name: &str, rest: &[Segment]) {
        if let Some(actual) = find_caseless(&at.path, name) {
            self.walk(at.enter(&actual, &actual), rest);
        }
    }

    fn enter_store_user(&mut self, at: &Cursor, id: &str, rest: &[Segment]) {
        let mut next = at.enter(id, STORE_USER_ID);
        next.store_user_id = Some(id.to_owned());
        if next.path.is_dir() {
            self.walk(next, rest);
        }
    }

    fn collect(&mut self, at: Cursor) {
        let Ok(meta) = fs::metadata(&at.path) else {
            return;
        };
        if meta.is_dir() {
            for name in children(&at.path) {
                self.collect(at.enter(&name, &name));
            }
        } else if !self.filter.excludes(&at.rel) {
            let file = self
                .found
                .entry(at.path.clone())
                .or_insert_with(|| SaveFile {
                    root: self.root,
                    rel: at.rel,
                    tags: Vec::new(),
                    store_user_id: at.store_user_id,
                    size: meta.len(),
                    modified: unix_mtime(&meta),
                    source: at.path,
                });
            for tag in self.tags {
                if !file.tags.contains(tag) {
                    file.tags.push(*tag);
                }
            }
        }
    }
}

fn children(dir: &Path) -> impl Iterator<Item = String> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
}

fn find_caseless(dir: &Path, name: &str) -> Option<String> {
    if dir.join(name).exists() {
        return Some(name.to_owned());
    }
    children(dir).find(|child| child.eq_ignore_ascii_case(name))
}

pub fn unix_mtime(meta: &Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |age| age.as_secs())
}
