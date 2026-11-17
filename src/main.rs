mod backup;
mod cli;
mod manifest;
mod prefix;
mod scan;
mod template;
mod ui;

use std::io::stdout;
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::Parser;

use backup::ImportOptions;
use cli::{Cli, Command, ListTarget, ManifestArgs, OutputArgs, SelectArgs, Target, WineArgs};
use prefix::{HostDirs, Roots};
use scan::GameSaves;
use ui::{ListHeader, ListStyle, Severity};

enum Outcome {
    Done,
    NothingFound,
}

impl Outcome {
    fn of(games: &[GameSaves]) -> Self {
        if games.is_empty() {
            Self::NothingFound
        } else {
            Self::Done
        }
    }
}

impl From<Outcome> for ExitCode {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Done => ExitCode::SUCCESS,
            Outcome::NothingFound => ExitCode::from(2),
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(outcome) => outcome.into(),
        Err(err) => {
            ui::print_report(Severity::Error, &err);
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<Outcome> {
    match cli.command {
        Command::List {
            source,
            wine,
            select,
            output,
        } => match source.target() {
            ListTarget::Saves(target) => {
                let (roots, found) = scan_saves(&cli.manifest, &target, &wine, &select)?;
                let header = ListHeader {
                    title: target.label(),
                    note: roots.wine_user().map(str::to_owned),
                };
                report(found, &header, &select, &output)
            }
            ListTarget::Backup(from) => {
                let found = backup::load(from, &select.filter(&wine))?;
                let header = ListHeader {
                    title: ui::tilde(from),
                    note: Some("backup".to_owned()),
                };
                report(found, &header, &select, &output)
            }
        },
        Command::Export {
            target,
            out,
            wine,
            select,
            output,
            transfer,
        } => {
            let target = target.target();
            let (_, found) = scan_saves(&cli.manifest, &target, &wine, &select)?;
            backup::export(&found, &out, transfer.progress)?;
            let header = ListHeader {
                title: format!("exported from {} to {}", target.label(), ui::tilde(&out)),
                note: None,
            };
            report(found, &header, &select, &output)
        }
        Command::Import {
            from,
            target,
            force,
            dry_run,
            wine,
            select,
            output,
            transfer,
        } => {
            let target = target.target();
            let roots = open_roots(&target, &wine, &select)?;
            let found = backup::load(&from, &select.filter(&wine))?;
            let options = ImportOptions {
                force,
                dry_run,
                store_user_id: wine.store_user_id.as_deref(),
                progress: transfer.progress,
            };
            let placed = backup::import(&found, &roots, &options)?;
            let header = ListHeader {
                title: format!(
                    "{} from {} to {}",
                    if dry_run { "would import" } else { "imported" },
                    ui::tilde(&from),
                    target.label()
                ),
                note: None,
            };
            let outcome = report(found, &header, &select, &output)?;
            if !output.json {
                ui::print_placements(&placed, dry_run);
            }
            Ok(outcome)
        }
    }
}

fn open_roots(target: &Target, wine: &WineArgs, select: &SelectArgs) -> Result<Roots> {
    if wine.install_dir.is_some() && select.game.len() != 1 {
        bail!("--install-dir needs exactly one --game");
    }
    let host = HostDirs::new(wine.install_dir.clone(), wine.store_root.clone());
    Ok(match target {
        Target::Prefix(prefix) => Roots::wine(prefix, wine.wine_user.as_deref(), host)?,
        Target::Native => Roots::native(host),
    })
}

fn scan_saves(
    source: &ManifestArgs,
    target: &Target,
    wine: &WineArgs,
    select: &SelectArgs,
) -> Result<(Roots, Vec<GameSaves>)> {
    let roots = open_roots(target, wine, select)?;
    let manifest = manifest::load(source.path.as_deref(), source.refresh)?;
    let found = scan::scan(&manifest, &roots, &select.filter(wine));
    Ok((roots, found))
}

fn report(
    mut games: Vec<GameSaves>,
    header: &ListHeader,
    select: &SelectArgs,
    output: &OutputArgs,
) -> Result<Outcome> {
    output.sort.apply(&mut games);
    if output.json {
        serde_json::to_writer_pretty(stdout(), &games)?;
        println!();
    } else {
        let style = ListStyle {
            long: output.long,
            tags: select.tags.len() > 1,
        };
        ui::print_list(header, &games, &style);
    }
    Ok(Outcome::of(&games))
}
