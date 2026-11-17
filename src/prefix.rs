use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use etcetera::BaseStrategy;
use thiserror::Error;

use crate::manifest::Os;
use crate::template::Root;
use crate::ui::tilde;

const PUBLIC_USER: &str = "Public";

#[derive(Debug, Error)]
pub enum PrefixError {
    #[error("this isn't a wine prefix")]
    NoDriveC { prefix: PathBuf },
    #[error("can't read the prefix users")]
    Unreadable { path: PathBuf, source: io::Error },
    #[error("the prefix has no wine users")]
    NoUsers { prefix: PathBuf },
    #[error("several wine users in this prefix")]
    AmbiguousUser { prefix: PathBuf, users: Vec<String> },
    #[error("no wine user named {wanted}")]
    UnknownUser { wanted: String, users: Vec<String> },
}

impl PrefixError {
    pub fn details(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::NoDriveC { prefix } | Self::NoUsers { prefix } => vec![("prefix", tilde(prefix))],
            Self::Unreadable { path, source } => {
                vec![("path", tilde(path)), ("cause", source.to_string())]
            }
            Self::AmbiguousUser { prefix, users } => {
                vec![("prefix", tilde(prefix)), ("users", users.join(", "))]
            }
            Self::UnknownUser { users, .. } => vec![("users", users.join(", "))],
        }
    }

    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::NoDriveC { .. } => {
                Some("point --prefix at the dir holding drive_c or at a proton compatdata dir")
            }
            Self::NoUsers { .. } => Some("run anything in the prefix once so wine sets it up"),
            Self::AmbiguousUser { .. } => Some("pick one with --wine-user <name>"),
            Self::UnknownUser { .. } | Self::Unreadable { .. } => None,
        }
    }
}

pub struct HostDirs {
    install_dir: Option<PathBuf>,
    store_root: Option<PathBuf>,
    home: Option<PathBuf>,
    xdg_data: Option<PathBuf>,
    xdg_config: Option<PathBuf>,
}

impl HostDirs {
    pub fn new(install_dir: Option<PathBuf>, store_root: Option<PathBuf>) -> Self {
        let xdg = etcetera::choose_base_strategy().ok();
        Self {
            install_dir,
            store_root,
            home: etcetera::home_dir().ok(),
            xdg_data: xdg.as_ref().map(BaseStrategy::data_dir),
            xdg_config: xdg.as_ref().map(BaseStrategy::config_dir),
        }
    }
}

struct WineHome {
    drive_c: PathBuf,
    user: String,
}

pub struct Roots {
    wine: Option<WineHome>,
    host: HostDirs,
}

impl Roots {
    pub fn native(host: HostDirs) -> Self {
        Self { wine: None, host }
    }

    pub fn wine(prefix: &Path, wanted: Option<&str>, host: HostDirs) -> Result<Self, PrefixError> {
        let drive_c = find_drive_c(prefix)?;
        let users_dir = drive_c.join("users");
        let users = wine_users(&users_dir)?;
        let user = match (wanted, users.as_slice()) {
            (Some(wanted), _) if users_dir.join(wanted).is_dir() => wanted.to_owned(),
            (Some(wanted), _) => {
                return Err(PrefixError::UnknownUser {
                    wanted: wanted.to_owned(),
                    users,
                });
            }
            (None, [only]) => only.clone(),
            (None, []) => {
                return Err(PrefixError::NoUsers {
                    prefix: prefix.to_owned(),
                });
            }
            (None, _) => {
                return Err(PrefixError::AmbiguousUser {
                    prefix: prefix.to_owned(),
                    users,
                });
            }
        };
        Ok(Self {
            wine: Some(WineHome { drive_c, user }),
            host,
        })
    }

    pub fn os(&self) -> Os {
        match self.wine {
            Some(_) => Os::Windows,
            None => Os::Linux,
        }
    }

    pub fn wine_user(&self) -> Option<&str> {
        self.wine.as_ref().map(|wine| wine.user.as_str())
    }

    pub fn resolve(&self, root: Root) -> Option<PathBuf> {
        match root {
            Root::LinuxHome => self.host.home.clone(),
            Root::XdgData => self.host.xdg_data.clone(),
            Root::XdgConfig => self.host.xdg_config.clone(),
            Root::Base => self.host.install_dir.clone(),
            Root::Store => self.host.store_root.clone(),
            windows => self.wine.as_ref()?.resolve(windows),
        }
    }
}

impl WineHome {
    fn resolve(&self, root: Root) -> Option<PathBuf> {
        let users = self.drive_c.join("users");
        let home = users.join(&self.user);
        Some(match root {
            Root::DriveC => self.drive_c.clone(),
            Root::Home => home,
            Root::WinDocuments => home.join("Documents"),
            Root::WinAppData => home.join("AppData/Roaming"),
            Root::WinLocalAppData => home.join("AppData/Local"),
            Root::WinProgramData => self.drive_c.join("ProgramData"),
            Root::WinPublic => users.join(PUBLIC_USER),
            Root::WinDir => self.drive_c.join("windows"),
            Root::LinuxHome | Root::XdgData | Root::XdgConfig | Root::Base | Root::Store => {
                return None;
            }
        })
    }
}

fn find_drive_c(prefix: &Path) -> Result<PathBuf, PrefixError> {
    ["drive_c", "pfx/drive_c"]
        .into_iter()
        .map(|sub| prefix.join(sub))
        .find(|path| path.is_dir())
        .ok_or_else(|| PrefixError::NoDriveC {
            prefix: prefix.to_owned(),
        })
}

fn wine_users(users_dir: &Path) -> Result<Vec<String>, PrefixError> {
    let entries = fs::read_dir(users_dir).map_err(|source| PrefixError::Unreadable {
        path: users_dir.to_owned(),
        source,
    })?;
    Ok(entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name != PUBLIC_USER)
        .collect())
}
