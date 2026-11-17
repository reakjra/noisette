use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use glob::Pattern;

use crate::manifest::Tag;
use crate::scan::{Filter, Sort};
use crate::ui::{ProgressMode, tilde};

const PREFIX_HELP: &str = "wine prefix holding drive_c or a proton compatdata dir";
const NATIVE_HELP: &str = "linux native saves in your home instead of a prefix";

#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    #[command(flatten)]
    pub manifest: ManifestArgs,
}

#[derive(Args)]
pub struct ManifestArgs {
    /// read the manifest from a local yaml file instead of fetching it
    #[arg(long = "manifest", global = true)]
    pub path: Option<PathBuf>,

    /// fetch the manifest again even if the cached copy is current
    #[arg(long, global = true)]
    pub refresh: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// list games with saves in a prefix or a backup folder
    List {
        #[command(flatten)]
        source: ListSource,

        #[command(flatten)]
        wine: WineArgs,

        #[command(flatten)]
        select: SelectArgs,

        #[command(flatten)]
        output: OutputArgs,
    },

    /// copy saves into one backup folder per game
    Export {
        #[command(flatten)]
        target: TargetArgs,

        /// folder that recieves the game backups
        #[arg(long)]
        out: PathBuf,

        #[command(flatten)]
        wine: WineArgs,

        #[command(flatten)]
        select: SelectArgs,

        #[command(flatten)]
        output: OutputArgs,

        #[command(flatten)]
        transfer: TransferArgs,
    },

    /// restore saves from a backup folder into a prefix
    Import {
        /// backup folder holding one game or many
        #[arg(long)]
        from: PathBuf,

        #[command(flatten)]
        target: TargetArgs,

        /// overwrite existing files without keeping a .noisette-bak copy
        #[arg(long)]
        force: bool,

        /// show what would be written without touching the prefix
        #[arg(long)]
        dry_run: bool,

        #[command(flatten)]
        wine: WineArgs,

        #[command(flatten)]
        select: SelectArgs,

        #[command(flatten)]
        output: OutputArgs,

        #[command(flatten)]
        transfer: TransferArgs,
    },
}

#[derive(Args)]
pub struct TransferArgs {
    /// json prints one line per copied file on stderr
    #[arg(long, default_value = "bar")]
    pub progress: ProgressMode,
}

pub enum Target<'a> {
    Prefix(&'a Path),
    Native,
}

pub enum ListTarget<'a> {
    Saves(Target<'a>),
    Backup(&'a Path),
}

#[derive(Args)]
#[group(required = true, multiple = false)]
pub struct TargetArgs {
    #[arg(long, help = PREFIX_HELP)]
    prefix: Option<PathBuf>,

    #[arg(long, help = NATIVE_HELP)]
    native: bool,
}

impl Target<'_> {
    pub fn label(&self) -> String {
        match self {
            Self::Prefix(prefix) => tilde(prefix),
            Self::Native => "native saves".to_owned(),
        }
    }
}

impl TargetArgs {
    pub fn target(&self) -> Target<'_> {
        self.prefix
            .as_deref()
            .map_or(Target::Native, Target::Prefix)
    }
}

#[derive(Args)]
#[group(required = true, multiple = false)]
pub struct ListSource {
    #[arg(long, help = PREFIX_HELP)]
    prefix: Option<PathBuf>,

    #[arg(long, help = NATIVE_HELP)]
    native: bool,

    /// backup folder to list instead of saves
    #[arg(long)]
    from: Option<PathBuf>,
}

impl ListSource {
    pub fn target(&self) -> ListTarget<'_> {
        match (&self.from, &self.prefix) {
            (Some(from), _) => ListTarget::Backup(from),
            (None, Some(prefix)) => ListTarget::Saves(Target::Prefix(prefix)),
            (None, None) => ListTarget::Saves(Target::Native),
        }
    }
}

#[derive(Args)]
pub struct WineArgs {
    /// user under drive_c/users (detected when there's only one)
    #[arg(long)]
    pub wine_user: Option<String>,

    /// game install dir for <base> paths (needs exactly one --game)
    #[arg(long, requires = "game")]
    pub install_dir: Option<PathBuf>,

    /// store install dir for <root> paths (like ~/.local/share/Steam)
    #[arg(long, requires = "game")]
    pub store_root: Option<PathBuf>,

    /// store user id in the prefix (any id when not given)
    #[arg(long)]
    pub store_user_id: Option<String>,
}

#[derive(Args)]
pub struct SelectArgs {
    /// manifest name or a steam/gog id (repeatable)
    #[arg(long)]
    pub game: Vec<String>,

    /// manifest tags to include
    #[arg(long, value_delimiter = ',', default_value = "save")]
    pub tags: Vec<Tag>,

    /// glob matched against root-relative paths (repeatable)
    #[arg(long)]
    pub exclude: Vec<Pattern>,
}

impl SelectArgs {
    pub fn filter<'a>(&'a self, wine: &'a WineArgs) -> Filter<'a> {
        Filter {
            games: &self.game,
            tags: &self.tags,
            exclude: &self.exclude,
            store_user_id: wine.store_user_id.as_deref(),
        }
    }
}

#[derive(Args)]
pub struct OutputArgs {
    /// print json for scripts and launchers
    #[arg(long)]
    pub json: bool,

    /// show every file under each game
    #[arg(short, long)]
    pub long: bool,

    #[arg(long, default_value = "name")]
    pub sort: Sort,
}
