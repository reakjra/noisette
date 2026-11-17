use std::collections::BTreeMap;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::ValueEnum;
use etcetera::BaseStrategy;
use serde::{Deserialize, Serialize};
use ureq::http::StatusCode;

use crate::ui::{self, Severity};

const MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/master/data/manifest.yaml";
const MAX_MANIFEST_BYTES: u64 = 256 * 1024 * 1024;
const CACHE_FORMAT: u32 = 2;

pub type Manifest = BTreeMap<String, Game>;

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Game {
    pub files: BTreeMap<String, FileRule>,
    pub steam: StoreRef,
    pub gog: StoreRef,
    pub id: ExtraIds,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExtraIds {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steam_extra: Vec<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gog_extra: Vec<u64>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FileRule {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<Condition>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Condition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<Os>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Tag {
    Save,
    Config,
    #[serde(other)]
    #[value(skip)]
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Windows,
    Linux,
    #[serde(other)]
    Other,
}

impl Os {
    const SUPPORTED: [Self; 2] = [Self::Windows, Self::Linux];
}

impl Tag {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Config => "config",
            Self::Other => "other",
        }
    }
}

impl Game {
    pub fn is_called(&self, name: &str, query: &str) -> bool {
        matches_game(name, self.store_ids(), query)
    }

    pub fn store_ids(&self) -> impl Iterator<Item = u64> {
        self.steam
            .id
            .into_iter()
            .chain(self.gog.id)
            .chain(self.id.steam_extra.iter().copied())
            .chain(self.id.gog_extra.iter().copied())
    }
}

pub fn matches_game(name: &str, mut store_ids: impl Iterator<Item = u64>, query: &str) -> bool {
    name.eq_ignore_ascii_case(query)
        || query
            .parse()
            .is_ok_and(|id: u64| store_ids.any(|own| own == id))
}

impl FileRule {
    pub fn tags_within(&self, wanted: &[Tag]) -> Vec<Tag> {
        self.tags
            .iter()
            .copied()
            .filter(|tag| wanted.contains(tag))
            .collect()
    }

    pub fn applies_to(&self, os: Os) -> bool {
        self.when.is_empty()
            || self
                .when
                .iter()
                .any(|cond| cond.os.is_none_or(|own| own == os))
    }
}

pub fn load(local: Option<&Path>, refresh: bool) -> Result<Manifest> {
    match local {
        Some(path) => {
            parse_yaml(&fs::read(path).with_context(|| format!("reading {}", path.display()))?)
        }
        None => Cache::open()?.load(refresh),
    }
}

fn parse_yaml(bytes: &[u8]) -> Result<Manifest> {
    let mut manifest: Manifest = serde_yaml_ng::from_slice(bytes).context("parsing manifest")?;
    for game in manifest.values_mut() {
        game.files
            .retain(|_, rule| Os::SUPPORTED.iter().any(|&os| rule.applies_to(os)));
    }
    manifest.retain(|_, game| !game.files.is_empty());
    Ok(manifest)
}

struct Cache {
    dir: PathBuf,
}

impl Cache {
    fn open() -> Result<Self> {
        let dir = etcetera::choose_base_strategy()?
            .cache_dir()
            .join("noisette");
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self { dir })
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join(format!("manifest-v{CACHE_FORMAT}.json"))
    }

    fn etag_path(&self) -> PathBuf {
        self.dir.join(format!("manifest-v{CACHE_FORMAT}.etag"))
    }

    fn load(&self, refresh: bool) -> Result<Manifest> {
        let etag = if refresh {
            None
        } else {
            fs::read_to_string(self.etag_path()).ok()
        };
        match self.fetch(etag.as_deref()) {
            Ok(Some(manifest)) => Ok(manifest),
            Ok(None) => self.read(),
            Err(err) if self.manifest_path().exists() => {
                ui::print_report(Severity::Warning, &err.context("using the cached manifest"));
                self.read()
            }
            Err(err) => Err(err),
        }
    }

    fn fetch(&self, etag: Option<&str>) -> Result<Option<Manifest>> {
        let mut request = ureq::get(MANIFEST_URL);
        if let Some(etag) = etag.filter(|_| self.manifest_path().exists()) {
            request = request.header("If-None-Match", etag);
        }
        let mut response = request.call().context("fetching manifest")?;
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        let new_etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_MANIFEST_BYTES)
            .read_to_vec()?;
        let manifest = parse_yaml(&bytes)?;
        fs::write(self.manifest_path(), serde_json::to_vec(&manifest)?)?;
        match new_etag {
            Some(etag) => fs::write(self.etag_path(), etag)?,
            None => fs::remove_file(self.etag_path()).or_else(ignore_missing)?,
        }
        Ok(Some(manifest))
    }

    fn read(&self) -> Result<Manifest> {
        let path = self.manifest_path();
        let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

fn ignore_missing(err: io::Error) -> io::Result<()> {
    match err.kind() {
        ErrorKind::NotFound => Ok(()),
        _ => Err(err),
    }
}
