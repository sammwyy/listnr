use anyhow::Result;
use crossbeam_channel::Receiver;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use listnr_core::{
    IndexReport, Library, ListnrConfig, PlaybackState, Player, PlayerCommand, PlayerConfig,
    PlayerEvent, PlaylistConfig, ThemeConfig, Track, cover_art,
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, Padding, Paragraph, Row, Table, TableState,
    },
};
use ratatui_image::{StatefulImage, picker::Picker, protocol::StatefulProtocol};
use std::{
    collections::BTreeMap,
    io::{self, Stdout},
    path::PathBuf,
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Content,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Tracks,
    Artists,
    Albums,
    Genres,
    Playlist(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupKind {
    Artist,
    Album,
    Genre,
}

#[derive(Clone)]
enum RowItem {
    Track(usize),
    Group {
        kind: GroupKind,
        name: String,
        count: usize,
    },
}

enum InputMode {
    Search(String),
    Playlist(String),
}

#[derive(Clone, Copy)]
struct Theme {
    background: Color,
    surface: Color,
    elevated: Color,
    accent: Color,
    text: Color,
    muted: Color,
    highlight: Color,
    error: Color,
}

#[derive(Default)]
struct UiAreas {
    navigation_hits: Vec<(Rect, usize)>,
    row_hits: Vec<(Rect, usize)>,
    previous: Rect,
    toggle: Rect,
    next: Rect,
    progress: Rect,
    volume: Rect,
}

impl From<&ThemeConfig> for Theme {
    fn from(value: &ThemeConfig) -> Self {
        Self {
            background: color(&value.background, Color::Rgb(11, 13, 15)),
            surface: color(&value.surface, Color::Rgb(18, 21, 24)),
            elevated: color(&value.elevated, Color::Rgb(26, 30, 34)),
            accent: color(&value.accent, Color::Rgb(30, 215, 96)),
            text: color(&value.text, Color::Rgb(244, 245, 246)),
            muted: color(&value.muted, Color::Rgb(139, 148, 158)),
            highlight: color(&value.highlight, Color::Rgb(39, 48, 43)),
            error: color(&value.error, Color::Rgb(255, 107, 107)),
        }
    }
}

struct App {
    tracks: Vec<Track>,
    config: ListnrConfig,
    config_path: PathBuf,
    player: Player,
    page: Page,
    focus: Focus,
    sidebar: usize,
    selected: usize,
    content_offset: usize,
    detail: Option<(GroupKind, String)>,
    search: String,
    input: Option<InputMode>,
    current: Option<usize>,
    playback: PlaybackState,
    position_ms: u64,
    duration_ms: Option<u64>,
    volume: f64,
    status: String,
    status_error: bool,
    indexer: Option<Receiver<Result<IndexReport>>>,
    picker: Picker,
    cover: Option<StatefulProtocol>,
    cover_path: Option<PathBuf>,
    now_cover: Option<StatefulProtocol>,
    now_cover_path: Option<PathBuf>,
    areas: UiAreas,
}

pub fn run(
    library: Library,
    cache: PathBuf,
    music_dirs: Vec<PathBuf>,
    config: ListnrConfig,
    config_path: PathBuf,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let result = run_terminal(&mut stdout, library, cache, music_dirs, config, config_path);
    disable_raw_mode()?;
    execute!(stdout, DisableMouseCapture, LeaveAlternateScreen)?;
    result
}

fn run_terminal(
    stdout: &mut Stdout,
    library: Library,
    cache: PathBuf,
    music_dirs: Vec<PathBuf>,
    config: ListnrConfig,
    config_path: PathBuf,
) -> Result<()> {
    // Terminal graphics are queried only after entering the alternate screen.
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)));
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.hide_cursor()?;
    let player = Player::spawn(PlayerConfig::default()).map_err(anyhow::Error::msg)?;
    let indexer = (!music_dirs.is_empty()).then(|| Library::index_in_background(cache, music_dirs));
    let mut app = App {
        tracks: library.tracks(None)?,
        config,
        config_path,
        player,
        page: Page::Home,
        focus: Focus::Content,
        sidebar: 0,
        selected: 0,
        content_offset: 0,
        detail: None,
        search: String::new(),
        input: None,
        current: None,
        playback: PlaybackState::Stopped,
        position_ms: 0,
        duration_ms: None,
        volume: 0.8,
        status: "Ready".into(),
        status_error: false,
        indexer,
        picker,
        cover: None,
        cover_path: None,
        now_cover: None,
        now_cover_path: None,
        areas: UiAreas::default(),
    };
    app.refresh_cover();
    let result = loop {
        app.poll_events();
        terminal.draw(|frame| draw(frame, &mut app))?;
        if event::poll(Duration::from_millis(80))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.handle_key(key)? {
                        break Ok(());
                    }
                }
                Event::Mouse(mouse) => app.handle_mouse(mouse)?,
                _ => {}
            }
        }
    };
    let _ = app.player.command(PlayerCommand::Quit);
    terminal.show_cursor()?;
    result
}

impl App {
    fn nav_len(&self) -> usize {
        5 + self.config.playlists.len()
    }

    fn activate_sidebar(&mut self) {
        self.page = match self.sidebar {
            0 => Page::Home,
            1 => Page::Tracks,
            2 => Page::Artists,
            3 => Page::Albums,
            4 => Page::Genres,
            value => Page::Playlist(value - 5),
        };
        self.detail = None;
        self.selected = 0;
        self.content_offset = 0;
        self.focus = Focus::Content;
        self.refresh_cover();
    }

    fn rows(&self) -> Vec<RowItem> {
        if let Some((kind, ref name)) = self.detail {
            return self
                .filtered_track_indices()
                .into_iter()
                .filter(|index| field(&self.tracks[*index], kind) == name)
                .map(RowItem::Track)
                .collect();
        }
        match self.page {
            Page::Home => self
                .filtered_track_indices()
                .into_iter()
                .rev()
                .take(50)
                .map(RowItem::Track)
                .collect(),
            Page::Tracks => self
                .filtered_track_indices()
                .into_iter()
                .map(RowItem::Track)
                .collect(),
            Page::Artists => self.group_rows(GroupKind::Artist),
            Page::Albums => self.group_rows(GroupKind::Album),
            Page::Genres => self.group_rows(GroupKind::Genre),
            Page::Playlist(index) => self
                .config
                .playlists
                .get(index)
                .map(|playlist| {
                    playlist
                        .tracks
                        .iter()
                        .filter_map(|path| self.tracks.iter().position(|track| &track.path == path))
                        .filter(|index| self.matches_search(&self.tracks[*index]))
                        .map(RowItem::Track)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn filtered_track_indices(&self) -> Vec<usize> {
        self.tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| self.matches_search(track))
            .map(|(index, _)| index)
            .collect()
    }

    fn matches_search(&self, track: &Track) -> bool {
        if self.search.is_empty() {
            return true;
        }
        let needle = self.search.to_lowercase();
        [
            Some(track.title.as_str()),
            track.artist.as_deref(),
            track.album.as_deref(),
            track.genre.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| value.to_lowercase().contains(&needle))
    }

    fn group_rows(&self, kind: GroupKind) -> Vec<RowItem> {
        let mut groups = BTreeMap::<String, usize>::new();
        for index in self.filtered_track_indices() {
            *groups
                .entry(field(&self.tracks[index], kind).to_owned())
                .or_default() += 1;
        }
        groups
            .into_iter()
            .map(|(name, count)| RowItem::Group { kind, name, count })
            .collect()
    }

    fn selected_track(&self) -> Option<usize> {
        match self.rows().get(self.selected) {
            Some(RowItem::Track(index)) => Some(*index),
            Some(RowItem::Group { kind, name, .. }) => self
                .tracks
                .iter()
                .position(|track| field(track, *kind) == name),
            None => None,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.focus == Focus::Sidebar {
            self.sidebar = move_index(self.sidebar, self.nav_len(), delta);
        } else {
            self.selected = move_index(self.selected, self.rows().len(), delta);
            self.refresh_cover();
        }
    }

    fn open_selected(&mut self) -> Result<()> {
        let rows = self.rows();
        match rows.get(self.selected) {
            Some(RowItem::Track(index)) => self.play(*index)?,
            Some(RowItem::Group { kind, name, .. }) => {
                self.detail = Some((*kind, name.clone()));
                self.selected = 0;
                self.content_offset = 0;
                self.refresh_cover();
            }
            None => {}
        }
        Ok(())
    }

    fn play(&mut self, index: usize) -> Result<()> {
        let track = &self.tracks[index];
        self.player.command(PlayerCommand::Play {
            path: Some(track.path.clone()),
        })?;
        self.current = Some(index);
        self.playback = PlaybackState::Playing;
        self.position_ms = 0;
        self.duration_ms = track.duration_ms;
        self.status = format!("Playing {}", track.title);
        self.status_error = false;
        self.load_now_cover(track.path.clone());
        Ok(())
    }

    fn skip(&mut self, delta: isize) -> Result<()> {
        let queue: Vec<usize> = self
            .rows()
            .iter()
            .filter_map(|row| {
                if let RowItem::Track(index) = row {
                    Some(*index)
                } else {
                    None
                }
            })
            .collect();
        if queue.is_empty() {
            return Ok(());
        }
        let current = self
            .current
            .and_then(|index| queue.iter().position(|candidate| *candidate == index))
            .unwrap_or(0);
        let next = move_index(current, queue.len(), delta);
        self.selected = next;
        self.play(queue[next])
    }

    fn add_to_playlist(&mut self) -> Result<()> {
        let Some(track_index) = self.selected_track().or(self.current) else {
            return Ok(());
        };
        let playlist_index = match self.page {
            Page::Playlist(index) => index,
            _ => 0,
        };
        if self.config.playlists.is_empty() {
            self.config.playlists.push(PlaylistConfig::default());
        }
        let last_playlist = self.config.playlists.len() - 1;
        let playlist = &mut self.config.playlists[playlist_index.min(last_playlist)];
        let path = self.tracks[track_index].path.clone();
        if !playlist.tracks.contains(&path) {
            playlist.tracks.push(path);
        }
        let name = playlist.name.clone();
        self.config.save(&self.config_path)?;
        self.status = format!("Added to {name}");
        self.status_error = false;
        Ok(())
    }

    fn finish_input(&mut self) -> Result<()> {
        match self.input.take() {
            Some(InputMode::Search(value)) => {
                self.search = value;
                self.selected = 0;
                self.content_offset = 0;
                self.refresh_cover();
            }
            Some(InputMode::Playlist(value)) if !value.trim().is_empty() => {
                self.config.playlists.push(PlaylistConfig {
                    name: value.trim().into(),
                    tracks: Vec::new(),
                });
                self.config.save(&self.config_path)?;
                self.sidebar = self.nav_len() - 1;
                self.activate_sidebar();
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_input(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => self.input = None,
            KeyCode::Enter => self.finish_input()?,
            KeyCode::Backspace => match self.input.as_mut() {
                Some(InputMode::Search(v) | InputMode::Playlist(v)) => {
                    v.pop();
                }
                None => {}
            },
            KeyCode::Char(character) => match self.input.as_mut() {
                Some(InputMode::Search(v) | InputMode::Playlist(v)) => v.push(character),
                None => {}
            },
            _ => {}
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if self.input.is_some() {
            self.handle_input(key)?;
            return Ok(false);
        }
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Tab => {
                self.focus = if self.focus == Focus::Sidebar {
                    Focus::Content
                } else {
                    Focus::Sidebar
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Sidebar,
            KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Content,
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Home | KeyCode::Char('g') => {
                if self.focus == Focus::Sidebar {
                    self.sidebar = 0
                } else {
                    self.selected = 0;
                    self.refresh_cover();
                }
            }
            KeyCode::End | KeyCode::Char('G') => {
                if self.focus == Focus::Sidebar {
                    self.sidebar = self.nav_len().saturating_sub(1)
                } else {
                    self.selected = self.rows().len().saturating_sub(1);
                    self.refresh_cover();
                }
            }
            KeyCode::Enter if self.focus == Focus::Sidebar => self.activate_sidebar(),
            KeyCode::Enter => self.open_selected()?,
            KeyCode::Esc | KeyCode::Backspace if self.detail.is_some() => {
                self.detail = None;
                self.selected = 0;
                self.content_offset = 0;
                self.refresh_cover();
            }
            KeyCode::Esc if !self.search.is_empty() => {
                self.search.clear();
                self.selected = 0;
                self.content_offset = 0;
                self.refresh_cover();
            }
            KeyCode::Char(' ') => self.player.command(PlayerCommand::Toggle)?,
            KeyCode::Char(']') => self.skip(1)?,
            KeyCode::Char('[') => self.skip(-1)?,
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.volume = (self.volume + 0.05).min(1.0);
                self.player
                    .command(PlayerCommand::Volume { value: self.volume })?;
            }
            KeyCode::Char('-') => {
                self.volume = (self.volume - 0.05).max(0.0);
                self.player
                    .command(PlayerCommand::Volume { value: self.volume })?;
            }
            KeyCode::Char('/') => self.input = Some(InputMode::Search(self.search.clone())),
            KeyCode::Char('N') => self.input = Some(InputMode::Playlist(String::new())),
            KeyCode::Char('a') => self.add_to_playlist()?,
            KeyCode::Char('D') => {
                if let Page::Playlist(index) = self.page {
                    if index < self.config.playlists.len() {
                        self.config.playlists.remove(index);
                        self.config.save(&self.config_path)?;
                        self.sidebar = 0;
                        self.activate_sidebar();
                        self.status = "Playlist deleted".into();
                    }
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<()> {
        let navigation = self
            .areas
            .navigation_hits
            .iter()
            .find(|(area, _)| contains(*area, mouse.column, mouse.row))
            .map(|(_, index)| *index);
        let content_row = self
            .areas
            .row_hits
            .iter()
            .find(|(area, _)| contains(*area, mouse.column, mouse.row))
            .map(|(_, index)| *index);
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_selection(-1),
            MouseEventKind::ScrollDown => self.move_selection(1),
            MouseEventKind::Down(MouseButton::Left) if navigation.is_some() => {
                if let Some(index) = navigation {
                    self.sidebar = index;
                    self.focus = Focus::Sidebar;
                    self.activate_sidebar();
                }
            }
            MouseEventKind::Down(MouseButton::Left) if content_row.is_some() => {
                if let Some(row) = content_row {
                    self.focus = Focus::Content;
                    self.selected = row;
                    self.refresh_cover();
                    self.open_selected()?;
                }
            }
            MouseEventKind::Down(MouseButton::Left)
                if contains(self.areas.previous, mouse.column, mouse.row) =>
            {
                self.skip(-1)?
            }
            MouseEventKind::Down(MouseButton::Left)
                if contains(self.areas.toggle, mouse.column, mouse.row) =>
            {
                self.player.command(PlayerCommand::Toggle)?
            }
            MouseEventKind::Down(MouseButton::Left)
                if contains(self.areas.next, mouse.column, mouse.row) =>
            {
                self.skip(1)?
            }
            MouseEventKind::Down(MouseButton::Left)
                if contains(self.areas.progress, mouse.column, mouse.row) =>
            {
                if let Some(duration) = self.duration_ms {
                    let ratio = mouse.column.saturating_sub(self.areas.progress.x) as f64
                        / self.areas.progress.width.max(1) as f64;
                    self.player.command(PlayerCommand::Seek {
                        position_ms: (duration as f64 * ratio.clamp(0.0, 1.0)) as u64,
                    })?;
                }
            }
            MouseEventKind::Down(MouseButton::Left)
                if contains(self.areas.volume, mouse.column, mouse.row) =>
            {
                self.volume = (mouse.column.saturating_sub(self.areas.volume.x) as f64
                    / self.areas.volume.width.max(1) as f64)
                    .clamp(0.0, 1.0);
                self.player
                    .command(PlayerCommand::Volume { value: self.volume })?;
            }
            _ => {}
        }
        Ok(())
    }

    fn poll_events(&mut self) {
        while let Ok(event) = self.player.events().try_recv() {
            match event {
                PlayerEvent::State { state } => self.playback = state,
                PlayerEvent::Position {
                    position_ms,
                    duration_ms,
                } => {
                    self.position_ms = position_ms;
                    self.duration_ms = duration_ms;
                }
                PlayerEvent::Volume { value } => self.volume = value,
                PlayerEvent::EndOfStream => {
                    let _ = self.skip(1);
                }
                PlayerEvent::Error { message } => {
                    self.status = message;
                    self.status_error = true;
                }
                _ => {}
            }
        }
        if let Some(receiver) = &self.indexer {
            if let Ok(result) = receiver.try_recv() {
                match result {
                    Ok(report) => {
                        self.status = format!(
                            "Library refreshed · +{} ~{} -{}",
                            report.added, report.updated, report.removed
                        );
                        self.status_error = false;
                    }
                    Err(error) => {
                        self.status = format!("Index failed: {error}");
                        self.status_error = true;
                    }
                }
                self.indexer = None;
            }
        }
    }

    fn refresh_cover(&mut self) {
        if let Some(index) = self.selected_track() {
            self.load_cover(self.tracks[index].path.clone());
        }
    }

    fn load_cover(&mut self, path: PathBuf) {
        if self.cover_path.as_ref() == Some(&path) {
            return;
        }
        self.cover_path = Some(path.clone());
        self.cover = cover_art(&path)
            .ok()
            .flatten()
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
            .map(|image| self.picker.new_resize_protocol(image));
    }

    fn load_now_cover(&mut self, path: PathBuf) {
        if self.now_cover_path.as_ref() == Some(&path) {
            return;
        }
        self.now_cover_path = Some(path.clone());
        self.now_cover = cover_art(&path)
            .ok()
            .flatten()
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
            .map(|image| self.picker.new_resize_protocol(image));
        self.load_cover(path);
    }
}

fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let theme = Theme::from(&app.config.theme);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        frame.area(),
    );
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(12), Constraint::Length(5)])
        .split(frame.area());
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24), Constraint::Min(45)])
        .split(vertical[0]);
    draw_sidebar(frame, body[0], app, theme);
    draw_content(frame, body[1], app, theme);
    draw_player(frame, vertical[1], app, theme);
    if let Some(input) = &app.input {
        draw_input(frame, input, theme);
    }
}

fn draw_sidebar(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    let border = if app.focus == Focus::Sidebar {
        theme.accent
    } else {
        theme.elevated
    };
    let sidebar_block = Block::default()
        .title(Line::from(vec![
            Span::styled(
                " Listnr ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("music", Style::default().fg(theme.muted)),
        ]))
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(border))
        .padding(Padding::new(1, 1, 1, 0));
    let navigation_area = sidebar_block.inner(area);
    app.areas.navigation_hits = (0..5)
        .map(|index| {
            (
                Rect::new(
                    navigation_area.x,
                    navigation_area.y.saturating_add(index as u16),
                    navigation_area.width,
                    1,
                ),
                index,
            )
        })
        .chain((0..app.config.playlists.len()).map(|playlist| {
            (
                Rect::new(
                    navigation_area.x,
                    navigation_area.y.saturating_add(6 + playlist as u16),
                    navigation_area.width,
                    1,
                ),
                playlist + 5,
            )
        }))
        .collect();
    let mut items = vec![
        ("⌂", "Home"),
        ("≡", "Tracks"),
        ("●", "Artists"),
        ("▣", "Albums"),
        ("#", "Genres"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (icon, label))| nav_item(icon, label, index == app.sidebar, theme))
    .collect::<Vec<_>>();
    items.push(ListItem::new(Line::from(Span::styled(
        "  ── PLAYLISTS ──",
        Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::BOLD),
    ))));
    for (index, playlist) in app.config.playlists.iter().enumerate() {
        items.push(nav_item(
            "♫",
            &playlist.name,
            app.sidebar == index + 5,
            theme,
        ));
    }
    frame.render_widget(
        List::new(items)
            .block(sidebar_block)
            .style(Style::default().bg(theme.surface).fg(theme.text)),
        area,
    );
}

fn nav_item<'a>(icon: &'a str, label: &'a str, selected: bool, theme: Theme) -> ListItem<'a> {
    let style = if selected {
        Style::default()
            .fg(theme.text)
            .bg(theme.highlight)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    ListItem::new(Line::from(vec![
        Span::styled(
            format!(" {icon}  "),
            Style::default().fg(if selected { theme.accent } else { theme.muted }),
        ),
        Span::styled(label, style),
    ]))
    .style(style)
}

fn draw_content(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    let show_hero = matches!(app.detail, Some((GroupKind::Artist | GroupKind::Album, _)));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(if show_hero { 10 } else { 3 }),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);
    if show_hero {
        draw_header(frame, chunks[0], app, theme);
    } else {
        draw_compact_header(frame, chunks[0], app, theme);
    }
    let rows = app.rows();
    let table_rows = rows
        .iter()
        .map(|row| match row {
            RowItem::Track(index) => {
                let track = &app.tracks[*index];
                let number = track
                    .track_number
                    .map(|value| format!("{value:02}"))
                    .unwrap_or_else(|| "–".into());
                Row::new(vec![
                    Cell::from(number).style(Style::default().fg(theme.muted)),
                    Cell::from(track.title.clone()).style(Style::default().fg(theme.text)),
                    Cell::from(
                        track
                            .artist
                            .clone()
                            .unwrap_or_else(|| "Unknown artist".into()),
                    )
                    .style(Style::default().fg(theme.muted)),
                    Cell::from(
                        track
                            .album
                            .clone()
                            .unwrap_or_else(|| "Unknown album".into()),
                    )
                    .style(Style::default().fg(theme.muted)),
                    Cell::from(
                        track
                            .duration_ms
                            .map(time)
                            .unwrap_or_else(|| "--:--".into()),
                    )
                    .style(Style::default().fg(theme.muted)),
                ])
            }
            RowItem::Group { name, count, .. } => Row::new(vec![
                Cell::from("●").style(Style::default().fg(theme.accent)),
                Cell::from(name.clone()).style(Style::default().fg(theme.text)),
                Cell::from(format!("{count} tracks")).style(Style::default().fg(theme.muted)),
                Cell::from(""),
                Cell::from(""),
            ]),
        })
        .collect::<Vec<_>>();
    let mut state = TableState::default()
        .with_offset(app.content_offset)
        .with_selected((!rows.is_empty()).then_some(app.selected.min(rows.len() - 1)));
    let border = if app.focus == Focus::Content {
        theme.accent
    } else {
        theme.elevated
    };
    let header = Row::new(["#", "TITLE", "ARTIST", "ALBUM", "TIME"]).style(
        Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::BOLD),
    );
    let widths = [
        Constraint::Length(4),
        Constraint::Percentage(42),
        Constraint::Percentage(24),
        Constraint::Percentage(24),
        Constraint::Length(7),
    ];
    frame.render_stateful_widget(
        Table::new(table_rows, widths)
            .header(header)
            .column_spacing(1)
            .row_highlight_style(Style::default().bg(theme.highlight).fg(theme.text))
            .highlight_symbol("▌ ")
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(border))
                    .padding(Padding::horizontal(1)),
            ),
        chunks[1],
        &mut state,
    );
    app.content_offset = state.offset();
    let rows_area = Rect {
        x: chunks[1].x.saturating_add(1),
        y: chunks[1].y.saturating_add(2),
        width: chunks[1].width.saturating_sub(2),
        height: chunks[1].height.saturating_sub(2),
    };
    app.areas.row_hits = (0..rows_area.height as usize)
        .filter_map(|visible| {
            let index = app.content_offset + visible;
            (index < rows.len()).then(|| {
                (
                    Rect::new(
                        rows_area.x,
                        rows_area.y.saturating_add(visible as u16),
                        rows_area.width,
                        1,
                    ),
                    index,
                )
            })
        })
        .collect();
    let status_color = if app.status_error {
        theme.error
    } else {
        theme.muted
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("  {}", app.status),
                Style::default().fg(status_color),
            ),
            Span::styled(shortcuts(app), Style::default().fg(theme.muted)),
        ]))
        .style(Style::default().bg(theme.background)),
        chunks[2],
    );
}

fn draw_compact_header(frame: &mut Frame<'_>, area: Rect, app: &App, theme: Theme) {
    let (title, subtitle) = page_title(app);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("  {title}"),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("   {subtitle}"), Style::default().fg(theme.muted)),
        ]))
        .block(Block::default().padding(Padding::vertical(1)))
        .style(Style::default().bg(theme.background)),
        area,
    );
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    let (title, subtitle) = page_title(app);
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(25),
            Constraint::Length(area.height.saturating_mul(2).min(20)),
        ])
        .split(area);
    let text = vec![
        Line::from(Span::styled(
            "COLLECTION",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            title,
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(subtitle, Style::default().fg(theme.muted))),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().padding(Padding::new(2, 1, 1, 0)))
            .style(Style::default().bg(theme.surface)),
        split[0],
    );
    if let Some(cover) = app.cover.as_mut() {
        frame.render_stateful_widget(
            StatefulImage::default(),
            split[1].inner(Margin {
                horizontal: 1,
                vertical: 1,
            }),
            cover,
        );
    } else {
        frame.render_widget(
            Paragraph::new("♪")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.accent).bg(theme.elevated))
                .block(Block::default().padding(Padding::vertical(2))),
            split[1].inner(Margin {
                horizontal: 1,
                vertical: 1,
            }),
        );
    }
}

fn draw_player(frame: &mut Frame<'_>, area: Rect, app: &mut App, theme: Theme) {
    frame.render_widget(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(theme.elevated))
            .style(Style::default().bg(theme.surface)),
        area,
    );
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(32),
            Constraint::Min(30),
            Constraint::Length(18),
        ])
        .split(inner);
    let left = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(7), Constraint::Min(8)])
        .split(columns[0]);
    if let Some(cover) = app.now_cover.as_mut() {
        frame.render_stateful_widget(StatefulImage::default(), left[0], cover);
    } else {
        frame.render_widget(
            Paragraph::new("♪")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.accent).bg(theme.elevated))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(theme.highlight)),
                ),
            left[0],
        );
    }
    let (title, artist) = app
        .current
        .map(|index| {
            (
                app.tracks[index].title.clone(),
                app.tracks[index]
                    .artist
                    .as_deref()
                    .unwrap_or("Unknown artist")
                    .to_owned(),
            )
        })
        .unwrap_or_else(|| ("Nothing playing".into(), "Choose a track".into()));
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                title,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(artist, Style::default().fg(theme.muted))),
        ])
        .block(Block::default().padding(Padding::new(2, 0, 0, 0))),
        left[1],
    );
    let icon = if app.playback == PlaybackState::Playing {
        "❚❚"
    } else {
        "▶"
    };
    let ratio = app
        .duration_ms
        .filter(|value| *value > 0)
        .map(|duration| (app.position_ms as f64 / duration as f64).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    let center = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(columns[1]);
    let controls = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Min(1),
        ])
        .split(center[0]);
    app.areas.previous = controls[1];
    app.areas.toggle = controls[2];
    app.areas.next = controls[3];
    frame.render_widget(
        Paragraph::new("◀")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted)),
        controls[1],
    );
    frame.render_widget(
        Paragraph::new(icon).alignment(Alignment::Center).style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        controls[2],
    );
    frame.render_widget(
        Paragraph::new("▶")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted)),
        controls[3],
    );
    let timeline = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(6),
            Constraint::Min(8),
            Constraint::Length(6),
        ])
        .split(center[1]);
    app.areas.progress = timeline[1];
    frame.render_widget(
        Paragraph::new(time(app.position_ms)).style(Style::default().fg(theme.muted)),
        timeline[0],
    );
    frame.render_widget(progress_bar(timeline[1].width, ratio, theme), timeline[1]);
    frame.render_widget(
        Paragraph::new(app.duration_ms.map(time).unwrap_or_else(|| "--:--".into()))
            .alignment(Alignment::Right)
            .style(Style::default().fg(theme.muted)),
        timeline[2],
    );
    frame.render_widget(
        Paragraph::new("[ / ] previous · next     Space play · pause")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted)),
        center[2],
    );
    let volume_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(columns[2]);
    frame.render_widget(
        Paragraph::new(format!("VOL  {:>3}%", (app.volume * 100.0).round() as u8))
            .alignment(Alignment::Right)
            .style(Style::default().fg(theme.muted)),
        volume_layout[0],
    );
    let volume_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(10)])
        .split(volume_layout[1]);
    app.areas.volume = volume_row[1];
    frame.render_widget(volume_meter(app.volume, theme), volume_row[1]);
}

fn draw_input(frame: &mut Frame<'_>, input: &InputMode, theme: Theme) {
    let area = centered_rect(56, 5, frame.area());
    let (title, value) = match input {
        InputMode::Search(value) => (" Search ", value),
        InputMode::Playlist(value) => (" New playlist ", value),
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!("{value}█"))
            .style(Style::default().fg(theme.text).bg(theme.elevated))
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.accent))
                    .padding(Padding::horizontal(1)),
            ),
        area,
    );
}

fn page_title(app: &App) -> (String, String) {
    if let Some((kind, name)) = &app.detail {
        return (
            name.clone(),
            format!("{} · {} tracks", group_label(*kind), app.rows().len()),
        );
    }
    match app.page {
        Page::Home => ("Good listening".into(), "Recently indexed tracks".into()),
        Page::Tracks => (
            "All tracks".into(),
            format!("{} songs in your library", app.rows().len()),
        ),
        Page::Artists => ("Artists".into(), format!("{} artists", app.rows().len())),
        Page::Albums => ("Albums".into(), format!("{} albums", app.rows().len())),
        Page::Genres => ("Genres".into(), format!("{} genres", app.rows().len())),
        Page::Playlist(index) => app
            .config
            .playlists
            .get(index)
            .map(|playlist| {
                (
                    playlist.name.clone(),
                    format!("Playlist · {} tracks", app.rows().len()),
                )
            })
            .unwrap_or(("Playlist".into(), "Empty".into())),
    }
}

fn shortcuts(app: &App) -> String {
    if !app.search.is_empty() {
        format!("   ·   Search: {}   Esc clear", app.search)
    } else {
        "   ·   / search   a add   N playlist   Space pause   q quit".into()
    }
}

fn progress_bar(width: u16, ratio: f64, theme: Theme) -> Paragraph<'static> {
    let filled = ((width as f64) * ratio.clamp(0.0, 1.0)).round() as usize;
    let empty = width as usize - filled.min(width as usize);
    Paragraph::new(Line::from(vec![
        Span::styled("━".repeat(filled), Style::default().fg(theme.accent)),
        Span::styled("─".repeat(empty), Style::default().fg(theme.highlight)),
    ]))
}

fn volume_meter(volume: f64, theme: Theme) -> Paragraph<'static> {
    const SEGMENTS: usize = 10;
    let filled = (volume.clamp(0.0, 1.0) * SEGMENTS as f64).floor() as usize;
    Paragraph::new(Line::from(vec![
        Span::styled("●".repeat(filled), Style::default().fg(theme.accent)),
        Span::styled(
            "·".repeat(SEGMENTS - filled),
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        ),
    ]))
}

fn field(track: &Track, kind: GroupKind) -> &str {
    match kind {
        GroupKind::Artist => track.artist.as_deref().unwrap_or("Unknown artist"),
        GroupKind::Album => track.album.as_deref().unwrap_or("Unknown album"),
        GroupKind::Genre => track.genre.as_deref().unwrap_or("Unknown genre"),
    }
}

fn group_label(kind: GroupKind) -> &'static str {
    match kind {
        GroupKind::Artist => "Artist",
        GroupKind::Album => "Album",
        GroupKind::Genre => "Genre",
    }
}

fn move_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (current as isize + delta).rem_euclid(len as isize) as usize
}

fn time(milliseconds: u64) -> String {
    format!(
        "{}:{:02}",
        milliseconds / 60_000,
        (milliseconds / 1_000) % 60
    )
}

fn color(value: &str, fallback: Color) -> Color {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 {
        return fallback;
    }
    u32::from_str_radix(hex, 16)
        .ok()
        .map(|rgb| Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
        .unwrap_or(fallback)
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x
        && x < area.x.saturating_add(area.width)
        && y >= area.y
        && y < area.y.saturating_add(area.height)
}
