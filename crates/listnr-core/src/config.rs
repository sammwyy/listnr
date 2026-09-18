use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

/// Persistent user settings. It intentionally contains no UI-specific state, so a
/// terminal client and a future desktop client share the same music libraries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ListnrConfig {
    /// Directories recursively scanned for supported audio files.
    pub libraries: Vec<PathBuf>,
    /// SQLite metadata cache. Relative paths are resolved from the current directory.
    pub cache: PathBuf,
    /// User-owned playlists, kept in this file so they are portable and editable.
    pub playlists: Vec<PlaylistConfig>,
    /// Colors used by terminal clients.
    pub theme: ThemeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistConfig {
    pub name: String,
    pub tracks: Vec<PathBuf>,
}

impl Default for PlaylistConfig {
    fn default() -> Self {
        Self {
            name: "Favorites".into(),
            tracks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub background: String,
    pub surface: String,
    pub elevated: String,
    pub accent: String,
    pub text: String,
    pub muted: String,
    pub highlight: String,
    pub error: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            background: "#0b0d0f".into(),
            surface: "#121518".into(),
            elevated: "#1a1e22".into(),
            accent: "#1ed760".into(),
            text: "#f4f5f6".into(),
            muted: "#8b949e".into(),
            highlight: "#27302b".into(),
            error: "#ff6b6b".into(),
        }
    }
}

impl Default for ListnrConfig {
    fn default() -> Self {
        let project = ProjectDirs::from("org", "listnr", "listnr");
        let home = directories::UserDirs::new()
            .map(|dirs| dirs.home_dir().to_owned())
            .unwrap_or_else(|| PathBuf::from("."));
        let music = directories::UserDirs::new()
            .and_then(|dirs| dirs.audio_dir().map(|path| path.to_owned()))
            .unwrap_or_else(|| home.join("Music"));
        let cache = project
            .map(|dirs| dirs.data_local_dir().join("cache.db"))
            .unwrap_or_else(|| PathBuf::from("cache.db"));
        Self {
            libraries: vec![music],
            cache,
            playlists: vec![PlaylistConfig::default()],
            theme: ThemeConfig::default(),
        }
    }
}

impl ListnrConfig {
    /// `$XDG_CONFIG_HOME/listnr/config.toml`, with a platform-appropriate fallback.
    pub fn default_path() -> PathBuf {
        ProjectDirs::from("org", "listnr", "listnr")
            .map(|dirs| dirs.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("config.toml"))
    }

    /// Reads configuration, writing a documented default file on first use.
    pub fn load_or_create(path: Option<PathBuf>) -> Result<(Self, PathBuf)> {
        let path = path.unwrap_or_else(Self::default_path);
        if path.exists() {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("reading configuration {}", path.display()))?;
            let config = toml::from_str(&text)
                .with_context(|| format!("parsing configuration {}", path.display()))?;
            return Ok((config, path));
        }
        let config = Self::default();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let contents = format!(
            "# listnr configuration\n# Add every directory that should be part of your music library.\n{}",
            toml::to_string_pretty(&config)?
        );
        fs::write(&path, contents)
            .with_context(|| format!("creating configuration {}", path.display()))?;
        Ok((config, path))
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let contents = format!(
            "# listnr configuration\n# Libraries, playlists, and UI colors are shared by every Listnr frontend.\n{}",
            toml::to_string_pretty(self)?
        );
        fs::write(path, contents)
            .with_context(|| format!("saving configuration {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_config_receives_theme_and_playlist_defaults() -> Result<()> {
        let config: ListnrConfig = toml::from_str("libraries = ['/music']\ncache = 'cache.db'\n")?;
        assert_eq!(config.playlists[0].name, "Favorites");
        assert_eq!(config.theme.accent, "#1ed760");
        Ok(())
    }

    #[test]
    fn playlists_round_trip_through_toml() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.toml");
        let mut config = ListnrConfig::default();
        config.playlists[0].tracks.push(PathBuf::from("song.flac"));
        config.save(&path)?;
        let (loaded, _) = ListnrConfig::load_or_create(Some(path))?;
        assert_eq!(loaded.playlists[0].tracks, [PathBuf::from("song.flac")]);
        Ok(())
    }
}
