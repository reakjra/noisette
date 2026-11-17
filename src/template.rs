use std::str::FromStr;

use anyhow::{Error, anyhow};
use glob::Pattern;
use serde::{Deserialize, Serialize};

use crate::manifest::Os;

pub const STORE_USER_ID: &str = "<storeUserId>";
const OS_USER_NAME: &str = "<osUserName>";
const STEAM_USERDATA: &str = "<root>/userdata/<storeUserId>/";
// not sure if there's more for ✨non-steam games✨
const STEAM_EMULATOR_SAVES: [&str; 2] = ["GSE Saves", "Goldberg SteamEmu Saves"];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(into = "&'static str", try_from = "String")]
pub enum Root {
    DriveC,
    Home,
    WinDocuments,
    WinAppData,
    WinLocalAppData,
    WinProgramData,
    WinPublic,
    WinDir,
    LinuxHome,
    XdgData,
    XdgConfig,
    Base,
    Store,
}

impl Root {
    const ALL: [Self; 13] = [
        Self::DriveC,
        Self::Home,
        Self::WinDocuments,
        Self::WinAppData,
        Self::WinLocalAppData,
        Self::WinProgramData,
        Self::WinPublic,
        Self::WinDir,
        Self::LinuxHome,
        Self::XdgData,
        Self::XdgConfig,
        Self::Base,
        Self::Store,
    ];

    const DRIVE_ALIASES: [(&str, Self); 5] = [
        ("C:/Users/<osUserName>", Self::Home),
        ("C:/Users/Public", Self::WinPublic),
        ("C:/ProgramData", Self::WinProgramData),
        ("C:/Windows", Self::WinDir),
        ("C:", Self::DriveC),
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DriveC => "driveC",
            Self::Home => "home",
            Self::WinDocuments => "winDocuments",
            Self::WinAppData => "winAppData",
            Self::WinLocalAppData => "winLocalAppData",
            Self::WinProgramData => "winProgramData",
            Self::WinPublic => "winPublic",
            Self::WinDir => "winDir",
            Self::LinuxHome => "linuxHome",
            Self::XdgData => "xdgData",
            Self::XdgConfig => "xdgConfig",
            Self::Base => "base",
            Self::Store => "storeRoot",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::LinuxHome => "home",
            Self::Store => "root",
            other => other.as_str(),
        }
    }

    fn serves(self, os: Os) -> bool {
        match self {
            Self::Base | Self::Store => true,
            Self::LinuxHome | Self::XdgData | Self::XdgConfig => os == Os::Linux,
            _ => os == Os::Windows,
        }
    }

    pub fn flag(self) -> Option<&'static str> {
        match self {
            Self::Base => Some("--install-dir"),
            Self::Store => Some("--store-root"),
            _ => None,
        }
    }

    fn strip_from(raw: &str, os: Os) -> Option<(Self, &str)> {
        let aliased = Self::DRIVE_ALIASES.iter().find_map(|&(alias, root)| {
            let head = raw.get(..alias.len())?;
            let rest = &raw[alias.len()..];
            let matched = head.eq_ignore_ascii_case(alias) && ends_segment(rest) && root.serves(os);
            matched.then_some((root, rest))
        });
        aliased.or_else(|| {
            Self::ALL
                .into_iter()
                .filter(|root| root.serves(os))
                .find_map(|root| {
                    let rest = raw
                        .strip_prefix('<')?
                        .strip_prefix(root.placeholder())?
                        .strip_prefix('>')?;
                    ends_segment(rest).then_some((root, rest))
                })
        })
    }
}

fn ends_segment(rest: &str) -> bool {
    rest.is_empty() || rest.starts_with('/')
}

impl From<Root> for &'static str {
    fn from(root: Root) -> Self {
        root.as_str()
    }
}

impl FromStr for Root {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|root| root.as_str() == s)
            .ok_or_else(|| anyhow!("unknown root {s}"))
    }
}

impl TryFrom<String> for Root {
    type Error = Error;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

pub enum Segment {
    Literal(String),
    Pattern(Pattern),
    AnyDepth,
    StoreUserId,
    OsUserName,
}

impl Segment {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "**" => Some(Self::AnyDepth),
            STORE_USER_ID => Some(Self::StoreUserId),
            OS_USER_NAME => Some(Self::OsUserName),
            _ => {
                let raw = raw.replace(STORE_USER_ID, "*");
                if raw.contains('<') {
                    None
                } else if raw.contains(['*', '?', '[']) {
                    Pattern::new(&raw).ok().map(Self::Pattern)
                } else {
                    Some(Self::Literal(raw))
                }
            }
        }
    }
}

pub struct Template {
    pub root: Root,
    pub segments: Vec<Segment>,
}

impl Template {
    pub fn expand(raw: &str, os: Os) -> Vec<Self> {
        let emulated = raw
            .strip_prefix(STEAM_USERDATA)
            .filter(|rest| {
                rest.split('/')
                    .next()
                    .is_some_and(|app| app.parse::<u32>().is_ok())
            })
            .filter(|_| os == Os::Windows);
        let emulated = emulated.into_iter().flat_map(move |rest| {
            STEAM_EMULATOR_SAVES
                .iter()
                .filter_map(move |dir| Self::parse(&format!("<winAppData>/{dir}/{rest}"), os))
        });
        Self::parse(raw, os).into_iter().chain(emulated).collect()
    }

    fn parse(raw: &str, os: Os) -> Option<Self> {
        let (root, rest) = Root::strip_from(raw, os)?;
        let segments = rest
            .split('/')
            .filter(|s| !s.is_empty())
            .map(Segment::parse)
            .collect::<Option<_>>()?;
        Some(Self { root, segments })
    }
}
