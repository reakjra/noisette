use std::collections::BTreeMap;
use std::path::Path;

use anstream::{eprintln, println};
use anstyle::{AnsiColor, Style};
use clap::ValueEnum;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;
use terminal_size::{Width, terminal_size};

use crate::backup::Placement;
use crate::manifest::Tag;
use crate::prefix::PrefixError;
use crate::scan::{GameSaves, SaveFile};

const ERROR: Style = AnsiColor::Red.on_default().bold();
const WARNING: Style = AnsiColor::Yellow.on_default().bold();
const HINT: Style = AnsiColor::Cyan.on_default();
const DIM: Style = Style::new().dimmed();
const BOLD: Style = Style::new().bold();

const GAME_ROW_RESERVED: usize = 42;
const FILE_ROW_RESERVED: usize = 49;
const TAGS_RESERVED: usize = 13;
const MIN_COLUMN: usize = 12;
const SIZE_UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
const PROGRESS_TEMPLATE: &str =
    "{bar:30.cyan/dim} {decimal_bytes:>9} / {decimal_total_bytes:<9} {wide_msg}";

pub struct ListHeader {
    pub title: String,
    pub note: Option<String>,
}

pub struct ListStyle {
    pub long: bool,
    pub tags: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ProgressMode {
    Bar,
    Json,
}

#[derive(Serialize)]
struct ProgressLine<'a> {
    game: &'a str,
    done: u64,
    total: u64,
}

enum ProgressSink {
    Bar(ProgressBar),
    Json,
}

pub struct Progress {
    sink: ProgressSink,
    game: String,
    done: u64,
    total: u64,
}

impl Progress {
    pub fn new(mode: ProgressMode, total: u64) -> Self {
        let sink = match mode {
            ProgressMode::Bar => ProgressSink::Bar(progress_bar(total)),
            ProgressMode::Json => ProgressSink::Json,
        };
        Self {
            sink,
            game: String::new(),
            done: 0,
            total,
        }
    }

    pub fn start(&mut self, game: &str) {
        game.clone_into(&mut self.game);
        if let ProgressSink::Bar(bar) = &self.sink {
            bar.set_message(game.to_owned());
        }
    }

    pub fn advance(&mut self, bytes: u64) {
        self.done += bytes;
        match &self.sink {
            ProgressSink::Bar(bar) => bar.inc(bytes),
            ProgressSink::Json => {
                let line = ProgressLine {
                    game: &self.game,
                    done: self.done,
                    total: self.total,
                };
                if let Ok(json) = serde_json::to_string(&line) {
                    eprintln!("{json}");
                }
            }
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        if let ProgressSink::Bar(bar) = &self.sink {
            bar.finish_and_clear();
        }
    }
}

fn progress_bar(total: u64) -> ProgressBar {
    let bar = ProgressBar::with_draw_target(Some(total), ProgressDrawTarget::stderr());
    let style = ProgressStyle::with_template(PROGRESS_TEMPLATE)
        .expect("progress template is valid")
        .progress_chars("=> ");
    bar.set_style(style);
    bar
}

pub fn print_placements(placed: &BTreeMap<Placement, usize>, dry_run: bool) {
    println!();
    if dry_run {
        println!("  {HINT}dry run{HINT:#}  nothing was written");
    }
    for (placement, count) in placed {
        println!(
            "  {DIM}{:<11}{DIM:#} {}",
            placement.as_str(),
            plural(*count, "file")
        );
    }
    if placed.contains_key(&Placement::Replaced) {
        println!(
            "  {HINT}hint{HINT:#}  replaced files are kept next to the new ones as *.noisette-bak"
        );
    }
}

#[derive(Clone, Copy)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Self::Error => "error:",
            Self::Warning => "warning:",
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Error => ERROR,
            Self::Warning => WARNING,
        }
    }
}

pub fn print_report(severity: Severity, err: &anyhow::Error) {
    let (details, hint) = match err.downcast_ref::<PrefixError>() {
        Some(known) => (known.details(), known.hint()),
        None => (
            err.chain()
                .skip(1)
                .map(|cause| ("cause", cause.to_string()))
                .collect(),
            None,
        ),
    };
    let (style, label) = (severity.style(), severity.label());
    eprintln!();
    eprintln!("{style}{label}{style:#} {err}");
    let rows: Vec<_> = details
        .into_iter()
        .map(|(label, value)| (DIM, label, value))
        .chain(hint.map(|hint| (HINT, "hint", hint.to_owned())))
        .collect();
    let width = rows
        .iter()
        .map(|(_, label, _)| label.len())
        .max()
        .unwrap_or_default();
    for (style, label, value) in rows {
        eprintln!("  {style}{label:<width$}{style:#}  {value}");
    }
}

pub fn print_list(header: &ListHeader, games: &[GameSaves], style: &ListStyle) {
    let total = games.iter().map(GameSaves::total_size).sum();
    println!();
    let note = header
        .note
        .as_ref()
        .map(|note| format!(" {DIM}({note}){DIM:#}"))
        .unwrap_or_default();
    println!(
        "{BOLD}{}{BOLD:#}{note}  {}  {}",
        header.title,
        plural(games.len(), "game"),
        size(total)
    );
    println!();

    let width = column_width(
        games.iter().map(|game| game.name.as_str()),
        GAME_ROW_RESERVED,
    );
    for game in games {
        println!(
            "  {BOLD}{:<width$}{BOLD:#}  {:>9}  {:>9}  {DIM}{}{DIM:#}",
            fit(&game.name, width, Cut::End),
            plural(game.files.len(), "file"),
            size(game.total_size()),
            date(game.last_modified())
        );
        if style.long {
            print_files(game, style);
        }
    }
}

fn print_files(game: &GameSaves, style: &ListStyle) {
    let rels: Vec<_> = game
        .files
        .iter()
        .map(|file| file.rel.to_string_lossy())
        .collect();
    let reserved = FILE_ROW_RESERVED + if style.tags { TAGS_RESERVED } else { 0 };
    let rel_width = column_width(rels.iter().map(AsRef::as_ref), reserved);
    for (file, rel) in game.files.iter().zip(&rels) {
        println!(
            "    {DIM}{:<15}{DIM:#} {:<rel_width$}  {:>9}  {DIM}{}{DIM:#}{}",
            file.root.as_str(),
            fit(rel, rel_width, Cut::Start),
            size(file.size),
            date(file.modified),
            if style.tags {
                tag_list(file)
            } else {
                String::new()
            }
        );
    }
    for id in game.store_user_ids() {
        println!("    {DIM}{:<15}{DIM:#} {id}", "store id");
    }
    println!();
}

fn tag_list(file: &SaveFile) -> String {
    let names: Vec<_> = file.tags.iter().map(Tag::as_str).collect();
    format!("  {HINT}{}{HINT:#}", names.join(","))
}

pub fn tilde(path: &Path) -> String {
    etcetera::home_dir()
        .ok()
        .and_then(|home| Some(format!("~/{}", path.strip_prefix(home).ok()?.display())))
        .unwrap_or_else(|| path.display().to_string())
}

fn size(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < SIZE_UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} B"),
        _ => format!("{value:.1} {}", SIZE_UNITS[unit]),
    }
}

fn date(unix: u64) -> String {
    i64::try_from(unix)
        .ok()
        .filter(|&secs| secs > 0)
        .and_then(|secs| Timestamp::from_second(secs).ok())
        .map_or_else(
            || "-".to_owned(),
            |ts| {
                ts.to_zoned(TimeZone::system())
                    .strftime("%Y-%m-%d %H:%M")
                    .to_string()
            },
        )
}

fn plural(count: usize, word: &str) -> String {
    match count {
        1 => format!("1 {word}"),
        _ => format!("{count} {word}s"),
    }
}

enum Cut {
    Start,
    End,
}

fn fit(text: &str, width: usize, cut: Cut) -> String {
    let len = text.chars().count();
    if len <= width {
        return text.to_owned();
    }
    let keep = width.saturating_sub(3);
    match cut {
        Cut::Start => format!("...{}", text.chars().skip(len - keep).collect::<String>()),
        Cut::End => format!("{}...", text.chars().take(keep).collect::<String>()),
    }
}

fn column_width<'a>(texts: impl Iterator<Item = &'a str>, reserved: usize) -> usize {
    let longest = texts
        .map(|text| text.chars().count())
        .max()
        .unwrap_or_default();
    match terminal_size() {
        Some((Width(cols), _)) => {
            longest.min(usize::from(cols).saturating_sub(reserved).max(MIN_COLUMN))
        }
        None => longest,
    }
}
