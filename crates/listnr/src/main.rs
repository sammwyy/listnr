use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use listnr_core::{Library, ListnrConfig, Player, PlayerCommand, PlayerConfig, PlayerEvent};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};

mod tui;

#[derive(Parser, Debug)]
#[command(
    name = "listnr",
    version,
    about = "GStreamer music player for the terminal and embedding"
)]
struct Cli {
    /// Enable debug logs (logs are always sent to stderr).
    #[arg(short, long, global = true)]
    verbose: bool,
    /// Configuration TOML path. Defaults to $XDG_CONFIG_HOME/listnr/config.toml.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Override the `cache` value from the configuration. Use ':memory:' for no persistence.
    #[arg(long, global = true)]
    cache: Option<PathBuf>,
    /// Music directory to refresh in the background when starting the TUI. Repeatable.
    #[arg(long = "music-dir", global = true)]
    music_dirs: Vec<PathBuf>,
    /// Start the JSON-lines stdin/stdout protocol instead of the TUI.
    #[arg(long, global = true)]
    headless: bool,
    /// Emit events as JSON lines while a file is playing.
    #[arg(long, global = true)]
    raw: bool,
    #[command(subcommand)]
    command: Option<Command>,
    /// Play this file immediately.
    path: Option<PathBuf>,
}
#[derive(Subcommand, Debug)]
enum Command {
    Index {
        paths: Vec<PathBuf>,
    },
    List {
        #[arg(short, long)]
        search: Option<String>,
    },
    Library,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(if cli.verbose {
            "listnr=debug"
        } else {
            "listnr=warn"
        })
        .with_writer(io::stderr)
        .init();
    let (config, config_path) = ListnrConfig::load_or_create(cli.config.clone())?;
    tracing::debug!(path = %config_path.display(), "loaded Listnr configuration");
    let cache = cli.cache.clone().unwrap_or_else(|| config.cache.clone());
    let music_dirs = if cli.music_dirs.is_empty() {
        config.libraries.clone()
    } else {
        cli.music_dirs.clone()
    };
    if cli.headless {
        return headless();
    }
    match cli.command {
        Some(Command::Index { paths }) => {
            let paths = if paths.is_empty() { music_dirs } else { paths };
            let mut library = Library::open(&cache)?;
            let report = library.index(&paths)?;
            println!(
                "Indexed: {} added, {} updated, {} removed",
                report.added, report.updated, report.removed
            );
        }
        Some(Command::List { search }) => {
            for track in Library::open(&cache)?.tracks(search.as_deref())? {
                println!(
                    "{}\t{}\t{}",
                    track.artist.as_deref().unwrap_or("Unknown artist"),
                    track.album.as_deref().unwrap_or("Unknown album"),
                    track.title
                );
            }
        }
        Some(Command::Library) => {
            let s = Library::open(&cache)?.summary()?;
            println!(
                "{} tracks · {} artists · {} albums · {} genres",
                s.tracks, s.artists, s.albums, s.genres
            );
        }
        None => {
            if let Some(path) = cli.path {
                play_file(path, cli.raw)?;
            } else {
                tui::run(
                    Library::open(&cache)?,
                    cache,
                    music_dirs,
                    config,
                    config_path,
                )?;
            }
        }
    }
    Ok(())
}

fn play_file(path: PathBuf, raw: bool) -> Result<()> {
    let player = Player::spawn(PlayerConfig::default()).map_err(anyhow::Error::msg)?;
    player.command(PlayerCommand::Play { path: Some(path) })?;
    loop {
        let event = player.events().recv()?;
        if raw {
            println!("{}", serde_json::to_string(&event)?);
        } else {
            render_event(&event);
        }
        if matches!(event, PlayerEvent::EndOfStream | PlayerEvent::Error { .. }) {
            break;
        }
    }
    let _ = player.command(PlayerCommand::Quit);
    Ok(())
}

/// JSON-lines protocol: one PlayerCommand per stdin line; one PlayerEvent per stdout line.
fn headless() -> Result<()> {
    let player = Player::spawn(PlayerConfig::default()).map_err(anyhow::Error::msg)?;
    let events = player.events().clone();
    std::thread::spawn(move || {
        for event in &events {
            println!("{}", serde_json::to_string(&event).unwrap());
            let _ = io::stdout().flush();
        }
    });
    for line in io::stdin().lock().lines() {
        let command: PlayerCommand = serde_json::from_str(&line.context("reading stdin")?)
            .context("invalid PlayerCommand JSON")?;
        let quit = matches!(command, PlayerCommand::Quit);
        player.command(command)?;
        if quit {
            break;
        }
    }
    Ok(())
}
fn render_event(event: &PlayerEvent) {
    match event {
        PlayerEvent::Loading { track } => eprintln!("Playing: {}", track.title),
        PlayerEvent::Error { message } => eprintln!("Error: {message}"),
        _ => {}
    }
}
