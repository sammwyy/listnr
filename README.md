# Listnr - Terminal Music Player

A modern, modular terminal music player. Listnr combines a responsive terminal UI with a reusable Rust playback core, so the same library, cache, playlists, and player protocol can later be used by another frontend.

## Features

- 🎵 Metadata indexing for common tagged audio files, with GStreamer playback
- 📁 Recursive music-library scanning with a SQLite cache
- ⚡ Terminal UI, direct file playback, and a JSON-lines headless protocol
- 🎛️ Playback controls, seeking, volume, mouse support, and playlists
- ⌨️ Keyboard-driven browsing and search
- 🎨 Configurable theme plus embedded or directory album artwork
- 🖥️ Linux and Windows support (with the platform GStreamer runtime installed)

## Architecture

```
listnr/
├── crates/
│   ├── listnr-types/   # Serializable commands and playback events
│   ├── listnr-core/    # Library, cache, configuration, and playback engine
│   └── listnr/         # CLI, TUI, and headless entry point
├── Cargo.toml          # Rust workspace
└── README.md
```

`listnr-core` has no terminal dependency, which keeps it available for a future desktop client without duplicating playback logic.

## Installation

### Linux

Install Rust plus GStreamer development packages and plugins. On Debian/Ubuntu these generally include `libgstreamer1.0-dev`, `gstreamer1.0-plugins-base`, and `gstreamer1.0-plugins-good`.

```bash
cargo build --release
./target/release/listnr
```

### Windows

Install the Rust MSVC toolchain and the [GStreamer 1.x development and runtime packages](https://gstreamer.freedesktop.org/download/). Make the GStreamer `bin` directory available through `PATH`; for builds, expose its `pkgconfig` directory through `PKG_CONFIG_PATH` (or use the environment configuration supplied by the SDK). Then run:

```powershell
cargo build --release
.\target\release\listnr.exe
```

The configuration directory is resolved by the operating system: typically `%APPDATA%\listnr\config.toml` on Windows and `$XDG_CONFIG_HOME/listnr/config.toml` on Linux.

## Usage

```bash
listnr index                     # first scan of configured libraries
listnr library                   # cached counts
listnr list --search jazz        # search cached metadata
listnr ./player.mp3              # play one file
listnr                            # browse the cached library in the TUI
listnr --music-dir ~/Music       # browse now and refresh in the background
listnr --raw ./player.mp3        # JSON event stream on stdout
listnr --headless                # JSON-lines commands on stdin, events on stdout
```

Headless example:

```bash
printf '%s\n%s\n' '{"command":"play","path":"./player.mp3"}' '{"command":"quit"}' | listnr --headless
```

## Navigation and playback

- `Esc`: leave the UI.
- `↑`/`↓`, mouse, and scroll wheel: move through the library.
- `/`: search.
- `Space`: pause or resume.
- `[`/`]`: previous/next track.
- `N`: create a playlist; `a`: add the selected track; `D`: delete the open playlist.

Album artwork is read from embedded images or `cover.jpg`/`folder.jpg`. Kitty, Sixel, and iTerm2 graphics are detected automatically, with Unicode half-block rendering as a fallback.

## Configuration

Listnr creates `config.toml` on its first run. Its default library is your platform music directory. Add paths with `libraries`, choose the SQLite cache with `cache`, or use `--config PATH` for a separate profile and `--cache :memory:` for an ephemeral cache.

```toml
[[playlists]]
name = "Night drive"
tracks = ["/path/to/Music/track.flac"]

[theme]
background = "#0b0d0f"
surface = "#121518"
elevated = "#1a1e22"
accent = "#1ed760"
text = "#f4f5f6"
muted = "#8b949e"
highlight = "#27302b"
error = "#ff6b6b"
```
