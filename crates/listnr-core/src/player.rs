use crossbeam_channel::{Receiver, Sender, unbounded};
#[cfg(feature = "gstreamer")]
use listnr_types::{PlaybackState, Track};
use listnr_types::{PlayerCommand, PlayerEvent};
use std::thread;
#[cfg(feature = "gstreamer")]
use std::{path::PathBuf, time::Duration};

#[derive(Debug, Clone)]
pub struct PlayerConfig {
    pub volume: f64,
}
impl Default for PlayerConfig {
    fn default() -> Self {
        Self { volume: 0.8 }
    }
}

/// Handle for an isolated player process/thread. The core never reads a terminal.
pub struct Player {
    commands: Sender<PlayerCommand>,
    events: Receiver<PlayerEvent>,
}
impl Player {
    pub fn spawn(config: PlayerConfig) -> Result<Self, String> {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        thread::Builder::new()
            .name("listnr-player".into())
            .spawn(move || run(command_rx, event_tx, config))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands: command_tx,
            events: event_rx,
        })
    }
    pub fn command(
        &self,
        command: PlayerCommand,
    ) -> Result<(), crossbeam_channel::SendError<PlayerCommand>> {
        self.commands.send(command)
    }
    pub fn events(&self) -> &Receiver<PlayerEvent> {
        &self.events
    }
}

#[cfg(feature = "gstreamer")]
fn run(commands: Receiver<PlayerCommand>, events: Sender<PlayerEvent>, config: PlayerConfig) {
    use gst::prelude::*;
    use gstreamer as gst;
    if let Err(error) = gst::init() {
        let _ = events.send(PlayerEvent::Error {
            message: format!("GStreamer initialization failed: {error}"),
        });
        return;
    }
    let playbin = match gst::ElementFactory::make("playbin").build() {
        Ok(v) => v,
        Err(e) => {
            let _ = events.send(PlayerEvent::Error {
                message: format!("Cannot create GStreamer playbin: {e}"),
            });
            return;
        }
    };
    playbin.set_property("volume", config.volume);
    let bus = match playbin.bus() {
        Some(v) => v,
        None => {
            let _ = events.send(PlayerEvent::Error {
                message: "GStreamer playbin has no message bus".into(),
            });
            return;
        }
    };
    let _ = events.send(PlayerEvent::Ready);
    let mut state = PlaybackState::Stopped;
    let mut current: Option<PathBuf> = None;
    let mut last_position = std::time::Instant::now();
    loop {
        while let Ok(command) = commands.try_recv() {
            match command {
                PlayerCommand::Play { path: Some(path) } => {
                    let title = path
                        .file_stem()
                        .and_then(|x| x.to_str())
                        .unwrap_or("Unknown track")
                        .to_owned();
                    let track = Track {
                        id: 0,
                        path: path.clone(),
                        title,
                        artist: None,
                        album: None,
                        genre: None,
                        track_number: None,
                        duration_ms: None,
                        modified_unix_ms: 0,
                    };
                    match gst::glib::filename_to_uri(&path, None) {
                        Ok(uri) => {
                            playbin.set_property("uri", uri.as_str());
                            let _ = playbin.set_state(gst::State::Playing);
                            current = Some(path);
                            state = PlaybackState::Playing;
                            let _ = events.send(PlayerEvent::Loading { track });
                            let _ = events.send(PlayerEvent::State { state });
                        }
                        Err(e) => {
                            let _ = events.send(PlayerEvent::Error {
                                message: format!("Invalid media path: {e}"),
                            });
                        }
                    }
                }
                PlayerCommand::Play { path: None } => {
                    let _ = playbin.set_state(gst::State::Playing);
                    state = PlaybackState::Playing;
                    let _ = events.send(PlayerEvent::State { state });
                }
                PlayerCommand::Pause => {
                    let _ = playbin.set_state(gst::State::Paused);
                    state = PlaybackState::Paused;
                    let _ = events.send(PlayerEvent::State { state });
                }
                PlayerCommand::Stop => {
                    let _ = playbin.set_state(gst::State::Ready);
                    state = PlaybackState::Stopped;
                    let _ = events.send(PlayerEvent::State { state });
                }
                PlayerCommand::Toggle => {
                    let next = if state == PlaybackState::Playing {
                        gst::State::Paused
                    } else {
                        gst::State::Playing
                    };
                    let _ = playbin.set_state(next);
                    state = if state == PlaybackState::Playing {
                        PlaybackState::Paused
                    } else {
                        PlaybackState::Playing
                    };
                    let _ = events.send(PlayerEvent::State { state });
                }
                PlayerCommand::Seek { position_ms } => {
                    let _ = playbin.seek_simple(
                        gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                        gst::ClockTime::from_mseconds(position_ms),
                    );
                }
                PlayerCommand::Volume { value } => {
                    let clamped = value.clamp(0.0, 1.0);
                    playbin.set_property("volume", clamped);
                    let _ = events.send(PlayerEvent::Volume { value: clamped });
                }
                PlayerCommand::Next | PlayerCommand::Previous => {
                    let _ = events.send(PlayerEvent::Error {
                        message: "Queue navigation belongs to the frontend playlist".into(),
                    });
                }
                PlayerCommand::Quit => {
                    let _ = playbin.set_state(gst::State::Null);
                    return;
                }
            }
        }
        while let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(20)) {
            use gst::MessageView;
            match message.view() {
                MessageView::Eos(..) => {
                    state = PlaybackState::Stopped;
                    let _ = events.send(PlayerEvent::EndOfStream);
                    let _ = events.send(PlayerEvent::State { state });
                }
                MessageView::Error(error) => {
                    let _ = events.send(PlayerEvent::Error {
                        message: error.error().to_string(),
                    });
                }
                _ => {}
            }
        }
        if state == PlaybackState::Playing && last_position.elapsed() >= Duration::from_millis(500)
        {
            let position_ms = playbin
                .query_position::<gst::ClockTime>()
                .map(|v| v.mseconds())
                .unwrap_or(0);
            let duration_ms = playbin
                .query_duration::<gst::ClockTime>()
                .map(|v| v.mseconds());
            let _ = events.send(PlayerEvent::Position {
                position_ms,
                duration_ms,
            });
            last_position = std::time::Instant::now();
        }
        if current.is_none() {
            thread::sleep(Duration::from_millis(15));
        }
    }
}

#[cfg(not(feature = "gstreamer"))]
fn run(_commands: Receiver<PlayerCommand>, events: Sender<PlayerEvent>, _config: PlayerConfig) {
    let _ = events.send(PlayerEvent::Error {
        message: "listnr was built without the `gstreamer` feature".into(),
    });
}
