//! Stable, UI-neutral types shared by the player core, CLI, and future desktop app.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Track {
    pub id: i64,
    pub path: PathBuf,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub track_number: Option<u32>,
    pub duration_ms: Option<u64>,
    pub modified_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibrarySummary {
    pub tracks: u64,
    pub artists: u64,
    pub albums: u64,
    pub genres: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlayerEvent {
    Ready,
    Loading {
        track: Track,
    },
    State {
        state: PlaybackState,
    },
    Position {
        position_ms: u64,
        duration_ms: Option<u64>,
    },
    Volume {
        value: f64,
    },
    EndOfStream,
    LibraryIndexed {
        added: u64,
        updated: u64,
        removed: u64,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    Stopped,
    Paused,
    Playing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum PlayerCommand {
    Play { path: Option<PathBuf> },
    Pause,
    Stop,
    Toggle,
    Next,
    Previous,
    Seek { position_ms: u64 },
    Volume { value: f64 },
    Quit,
}
