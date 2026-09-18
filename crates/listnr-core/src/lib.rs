//! In-process music library and GStreamer playback engine.
//!
//! `Player` is deliberately independent of terminals and windows: callers send commands
//! and consume typed events. This makes the same core usable by the CLI and a future GUI.

mod config;
mod library;
mod player;

pub use config::{ListnrConfig, PlaylistConfig, ThemeConfig};
pub use library::{IndexReport, Library, cover_art};
pub use listnr_types::{LibrarySummary, PlaybackState, PlayerCommand, PlayerEvent, Track};
pub use player::{Player, PlayerConfig};
