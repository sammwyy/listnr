use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, unbounded};
use lofty::{
    file::{AudioFile, TaggedFileExt},
    picture::PictureType,
    probe::Probe,
    tag::Accessor,
};
use listnr_types::{LibrarySummary, Track};
use rusqlite::{Connection, params};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use walkdir::WalkDir;

#[derive(Debug, Default, Clone, Copy)]
pub struct IndexReport {
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
}

pub struct Library {
    connection: Connection,
}

impl Library {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        // SQLite creates the file but not its parent directory. This is needed for the
        // default XDG data path during Listnr's first run.
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating cache directory {}", parent.display()))?;
        }
        let connection = Connection::open(path).context("opening music cache")?;
        connection.execute_batch(
            "\
            CREATE TABLE IF NOT EXISTS tracks (
              id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, title TEXT NOT NULL,
              artist TEXT, album TEXT, genre TEXT, track_number INTEGER,
              duration_ms INTEGER, modified_unix_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS tracks_artist ON tracks(artist);
            CREATE INDEX IF NOT EXISTS tracks_album ON tracks(album);
            CREATE INDEX IF NOT EXISTS tracks_genre ON tracks(genre);",
        )?;
        Ok(Self { connection })
    }

    pub fn index(&mut self, roots: &[PathBuf]) -> Result<IndexReport> {
        let mut report = IndexReport::default();
        let mut seen = HashSet::new();
        for root in roots {
            for entry in WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file() || !is_audio(entry.path()) {
                    continue;
                }
                let path = entry
                    .path()
                    .canonicalize()
                    .unwrap_or_else(|_| entry.path().to_owned());
                seen.insert(path.to_string_lossy().into_owned());
                let modified = modified_ms(&path).unwrap_or(0);
                let old: Option<i64> = self
                    .connection
                    .query_row(
                        "SELECT modified_unix_ms FROM tracks WHERE path = ?1",
                        [&path.to_string_lossy()],
                        |r| r.get(0),
                    )
                    .ok();
                if old == Some(modified) {
                    continue;
                }
                match read_track(&path, modified) {
                    Ok(track) => {
                        if old.is_some() {
                            report.updated += 1;
                        } else {
                            report.added += 1;
                        }
                        self.upsert(&track)?;
                    }
                    Err(error) => {
                        tracing::debug!(path = %path.display(), %error, "skipping unreadable media file")
                    }
                }
            }
        }
        // Only remove stale rows underneath indexed roots; unrelated library roots are retained.
        let existing: Vec<(i64, String)> = self
            .connection
            .prepare("SELECT id, path FROM tracks")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, path) in existing {
            let p = Path::new(&path);
            if roots.iter().any(|root| p.starts_with(root)) && !seen.contains(&path) {
                self.connection
                    .execute("DELETE FROM tracks WHERE id = ?1", [id])?;
                report.removed += 1;
            }
        }
        Ok(report)
    }

    /// Refresh a persisted cache without blocking a UI. The worker owns a separate
    /// SQLite connection, while callers can keep rendering results from their cache.
    pub fn index_in_background(
        cache_path: PathBuf,
        roots: Vec<PathBuf>,
    ) -> Receiver<Result<IndexReport>> {
        let (sender, receiver) = unbounded();
        std::thread::Builder::new()
            .name("listnr-indexer".into())
            .spawn(move || {
                let result = Self::open(cache_path).and_then(|mut library| library.index(&roots));
                let _ = sender.send(result);
            })
            .expect("starting the music indexer thread");
        receiver
    }

    pub fn tracks(&self, filter: Option<&str>) -> Result<Vec<Track>> {
        let needle = filter.map(|f| format!("%{}%", f.to_lowercase()));
        let sql = if needle.is_some() {
            "SELECT id,path,title,artist,album,genre,track_number,duration_ms,modified_unix_ms FROM tracks WHERE lower(title) LIKE ?1 OR lower(COALESCE(artist,'')) LIKE ?1 OR lower(COALESCE(album,'')) LIKE ?1 ORDER BY artist,album,track_number,title"
        } else {
            "SELECT id,path,title,artist,album,genre,track_number,duration_ms,modified_unix_ms FROM tracks ORDER BY artist,album,track_number,title"
        };
        let mut statement = self.connection.prepare(sql)?;
        let rows = if let Some(n) = needle {
            statement.query_map([n], row_to_track)?
        } else {
            statement.query_map([], row_to_track)?
        };
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn summary(&self) -> Result<LibrarySummary> {
        let value = |sql: &str| -> Result<u64> {
            self.connection
                .query_row(sql, [], |r| r.get::<_, u64>(0))
                .map_err(anyhow::Error::from)
        };
        Ok(LibrarySummary {
            tracks: value("SELECT count(*) FROM tracks")?,
            artists: value("SELECT count(DISTINCT artist) FROM tracks WHERE artist IS NOT NULL")?,
            albums: value("SELECT count(DISTINCT album) FROM tracks WHERE album IS NOT NULL")?,
            genres: value("SELECT count(DISTINCT genre) FROM tracks WHERE genre IS NOT NULL")?,
        })
    }

    fn upsert(&self, track: &Track) -> Result<()> {
        self.connection.execute("INSERT INTO tracks(path,title,artist,album,genre,track_number,duration_ms,modified_unix_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(path) DO UPDATE SET title=excluded.title,artist=excluded.artist,album=excluded.album,genre=excluded.genre,track_number=excluded.track_number,duration_ms=excluded.duration_ms,modified_unix_ms=excluded.modified_unix_ms",
            params![track.path.to_string_lossy(),track.title,track.artist,track.album,track.genre,track.track_number,track.duration_ms,track.modified_unix_ms])?;
        Ok(())
    }
}

/// Returns embedded front-cover bytes, falling back to conventional files beside the track.
pub fn cover_art(path: &Path) -> Result<Option<Vec<u8>>> {
    if let Ok(file) = Probe::open(path).and_then(|probe| probe.read()) {
        let pictures = file.tags().iter().flat_map(|tag| tag.pictures());
        if let Some(picture) = pictures
            .clone()
            .find(|picture| picture.pic_type() == PictureType::CoverFront)
            .or_else(|| pictures.into_iter().next())
        {
            return Ok(Some(picture.data().to_vec()));
        }
    }
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let names = [
        "cover.jpg",
        "cover.png",
        "folder.jpg",
        "folder.png",
        "front.jpg",
        "front.png",
    ];
    if let Ok(entries) = fs::read_dir(directory) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if names.contains(&name.as_str()) && entry.path().is_file() {
                return fs::read(entry.path()).map(Some).map_err(Into::into);
            }
        }
    }
    Ok(None)
}

fn row_to_track(row: &rusqlite::Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        title: row.get(2)?,
        artist: row.get(3)?,
        album: row.get(4)?,
        genre: row.get(5)?,
        track_number: row.get(6)?,
        duration_ms: row.get(7)?,
        modified_unix_ms: row.get(8)?,
    })
}
fn is_audio(path: &Path) -> bool {
    matches!(path.extension().and_then(|x| x.to_str()).map(|x| x.to_ascii_lowercase()), Some(ref x) if ["mp3","flac","ogg","opus","m4a","aac","wav","wma","aiff"].contains(&x.as_str()))
}
fn modified_ms(path: &Path) -> Result<i64> {
    Ok(fs::metadata(path)?
        .modified()?
        .duration_since(UNIX_EPOCH)?
        .as_millis() as i64)
}
fn read_track(path: &Path, modified_unix_ms: i64) -> Result<Track> {
    let file = Probe::open(path)?.read()?;
    let tag = file.primary_tag().or_else(|| file.first_tag());
    let fallback = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown track")
        .to_owned();
    Ok(Track {
        id: 0,
        path: path.to_owned(),
        title: tag
            .and_then(|t| t.title())
            .map(|value| value.into_owned())
            .unwrap_or(fallback),
        artist: tag.and_then(|t| t.artist()).map(|value| value.into_owned()),
        album: tag.and_then(|t| t.album()).map(|value| value.into_owned()),
        genre: tag.and_then(|t| t.genre()).map(|value| value.into_owned()),
        track_number: tag.and_then(|t| t.track()),
        duration_ms: Some(file.properties().duration().as_millis() as u64),
        modified_unix_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_persistent_empty_library() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let cache = directory.path().join("cache.db");
        assert_eq!(Library::open(&cache)?.summary()?.tracks, 0);
        // Opening the cache again verifies schema creation is idempotent and persisted.
        assert_eq!(Library::open(&cache)?.tracks(None)?.len(), 0);
        Ok(())
    }
}
