//! App state and what the backend's events do to it. No drawing here.
//!
//! One link is one session. A session can need more than one backend run: the first run
//! downloads the tracks the user does not have yet, and asks about the others (tracks already in
//! the folder). Each "download again" answer queues the track for a follow-up run. Questions
//! that get no answer are skipped after a countdown that starts when nothing is downloading.

use std::sync::{Arc, Mutex};

use rune::{AppHandle, TextEdit};

use crate::account::{self, Account, Check};
use crate::backend::{self, Cand, Event, Run};
use crate::i18n::Lang;
use crate::gfx;
use crate::history::{self, History, Library, SavedTrack, Session};
use crate::media::Media;
use crate::notes::{self, Level, Note};
use crate::settings::{Settings, config_dir};
use crate::update::{self, Up};

/// Seconds the user has to answer "you already have these" once nothing else is downloading.
pub const ASK_SECS: f64 = 10.0;
/// The same when every track of the link is already there.
pub const ASK_ALL_SECS: f64 = 20.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TState {
    #[default]
    Pending,
    Searching,
    Downloading,
    Done,
    /// Not on Soulseek; the YouTube fallback may still find it.
    Failed,
    YtSearch,
    YtDownload,
    Missing,
    /// Already in the folder: waits for "download again" or "skip".
    Have,
    /// The user said "download again"; it goes in the next run.
    Queued,
    /// Already in the folder, and not downloaded again.
    Skipped,
}

impl TState {
    fn from(s: &str) -> Option<TState> {
        Some(match s {
            "searching" => TState::Searching,
            "downloading" => TState::Downloading,
            "done" => TState::Done,
            "failed" => TState::Failed,
            "yt_search" => TState::YtSearch,
            "yt_download" => TState::YtDownload,
            "missing" => TState::Missing,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub id: String,
    /// Seconds, from Spotify.
    pub duration: u32,
    pub state: TState,
    pub bytes: u64,
    pub total: u64,
    pub peer: String,
    pub youtube: bool,
    /// The file came from the track's own SoundCloud page.
    pub soundcloud: bool,
    pub url: String,
    pub bitrate: u32,
    /// The mp3 in the output folder, after the run.
    pub path: String,
    /// The file the user already has (for `Have` and `Skipped`).
    pub have: String,
    /// Unix seconds: when its download started and ended.
    pub started: f64,
    pub finished: f64,
    /// Part of the backend run that is going on.
    pub in_run: bool,
    /// The Soulseek files found for it, best first.
    pub cands: Vec<Cand>,
    /// No more candidates will come.
    pub settled: bool,
    /// The candidate it downloads (or downloaded).
    pub cand: Option<u32>,
    /// The file that grows while it downloads (for the preview player), and its format.
    pub part: String,
    pub format: String,
}

impl Track {
    pub fn new(artist: &str, title: &str) -> Track {
        Track { artist: artist.into(), title: title.into(), ..Default::default() }
    }

    /// Manual mode: the files are there and the track waits for the user's pick.
    pub fn choosing(&self, manual: bool) -> bool {
        manual && self.in_run && self.state == TState::Searching && !self.cands.is_empty() && self.cand.is_none()
    }

    /// The preview can play its file while it downloads (the formats the player reads).
    pub fn previewable(&self) -> bool {
        !self.part.is_empty() && matches!(self.format.as_str(), "mp3" | "flac" | "wav" | "aif" | "aiff")
    }

    /// The file was streamed (YouTube Music or SoundCloud), not sent by a Soulseek peer.
    pub fn streamed(&self) -> bool {
        self.youtube || self.soundcloud
    }

    /// How far this track is, 0 to 1.
    pub fn progress(&self) -> f32 {
        match self.state {
            TState::Pending | TState::Queued => 0.0,
            TState::Searching => 0.04,
            TState::Downloading => 0.06 + 0.94 * if self.total > 0 { self.bytes as f32 / self.total as f32 } else { 0.0 },
            TState::Failed | TState::YtSearch => 0.2,
            TState::YtDownload => 0.55,
            TState::Done | TState::Missing | TState::Have | TState::Skipped => 1.0,
        }
    }

    fn saved(&self) -> SavedTrack {
        let size = if self.streamed() || self.total == 0 { std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0) } else { self.total };
        SavedTrack {
            artist: self.artist.clone(),
            title: self.title.clone(),
            album: self.album.clone(),
            id: self.id.clone(),
            duration: self.duration,
            state: if self.state == TState::Done { "done" } else { "missing" }.into(),
            source: if self.soundcloud { "soundcloud" } else if self.youtube { "youtube" } else if self.state == TState::Done { "soulseek" } else { "" }.into(),
            peer: self.peer.clone(),
            url: self.url.clone(),
            bitrate: self.bitrate,
            size,
            path: self.path.clone(),
            started: self.started,
            finished: self.finished,
        }
    }
}

/// What the details panel of a track shows, from the run or from the history.
#[derive(Clone, Debug, Default)]
pub struct Detail {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: u32,
    /// "DOWNLOADED", "DOWNLOADING 64%", "NOT FOUND", …
    pub status: String,
    pub source: String,
    pub peer: String,
    pub url: String,
    pub bitrate: u32,
    pub size: u64,
    /// The file to play, or empty.
    pub path: String,
    pub started: f64,
    pub finished: f64,
    /// An image file to show when the mp3 has no picture.
    pub cover: String,
    /// The file is gone from the folder.
    pub moved: bool,
    /// Something went wrong (not found, moved): the status shows in red.
    pub bad: bool,
    /// `path` is the file that is still downloading (a preview), not the mp3.
    pub preview: bool,
}

impl Detail {
    /// Bytes per second over the whole download.
    pub fn speed(&self) -> f64 {
        let t = self.finished - self.started;
        if self.size > 0 && t > 0.5 { self.size as f64 / t } else { 0.0 }
    }

    pub fn took(&self) -> f64 {
        if self.finished > self.started && self.started > 0.0 { self.finished - self.started } else { 0.0 }
    }

    fn from_saved(t: &SavedTrack, cover: &str, l: Lang) -> Detail {
        let moved = t.state == "done" && !t.path.is_empty() && !std::path::Path::new(&t.path).is_file();
        Detail {
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration: t.duration,
            status: if t.state == "done" {
                if moved { l.t("FILE MOVED OR DELETED", "EL ARCHIVO SE MOVIÓ O SE BORRÓ") } else { l.t("DOWNLOADED", "DESCARGADA") }
            } else {
                l.t("NOT FOUND", "NO ENCONTRADA")
            }
            .into(),
            source: t.source.clone(),
            peer: t.peer.clone(),
            url: t.url.clone(),
            bitrate: t.bitrate,
            size: t.size,
            path: if moved { String::new() } else { t.path.clone() },
            started: t.started,
            finished: t.finished,
            cover: cover.to_string(),
            moved,
            bad: moved || t.state != "done",
            preview: false,
        }
    }
}

/// The fields that belong to one tab. The open tab's are in [`State`] itself (so the screens
/// read them as before); the others wait in `State::tabs` and are swapped in to show them or to
/// take an event of their run.
macro_rules! tab_fields {
    ($m:ident) => {
        $m! {
            link: TextEdit,
            phase: Phase,
            stage: String,
            name: String,
            kind: String,
            tracks: Vec<Track>,
            convert: (usize, usize),
            files: usize,
            out_dir: String,
            error: String,
            notice: String,
            last_log: String,
            speed: f32,
            speed_bytes: u64,
            speed_t: f64,
            login_problem: bool,
            run: Option<Run>,
            run_id: u64,
            next_only: Option<Vec<usize>>,
            running_link: String,
            demo_t0: Option<f64>,
            library: Arc<Mutex<Option<Library>>>,
            session_id: String,
            session_started: f64,
            cover: String,
            open: Option<String>,
            countdown: Option<(f64, f64)>,
            show_history: bool,
            stopping: bool,
            last_progress: f64,
            stopped_early: bool,
            manual: bool,
            notes: Vec<Note>,
            soulseek_down: bool,
            hover_started: Option<usize>,
            scrub: Option<(String, f32)>
        }
    };
}

macro_rules! define_tab {
    ($($f:ident: $t:ty),*) => {
        /// A tab that is not the open one (see `tab_fields`).
        pub struct Tab {
            $(pub $f: $t,)*
        }

        impl Tab {
            fn swap_with(&mut self, s: &mut State) {
                $(std::mem::swap(&mut self.$f, &mut s.$f);)*
            }
        }
    };
}
tab_fields!(define_tab);

impl Default for Tab {
    fn default() -> Tab {
        Tab {
            link: TextEdit::new(""),
            phase: Phase::Idle,
            stage: String::new(),
            name: String::new(),
            kind: String::new(),
            tracks: Vec::new(),
            convert: (0, 0),
            files: 0,
            out_dir: String::new(),
            error: String::new(),
            notice: String::new(),
            last_log: String::new(),
            speed: 0.0,
            speed_bytes: 0,
            speed_t: 0.0,
            login_problem: false,
            run: None,
            run_id: 0,
            next_only: None,
            running_link: String::new(),
            demo_t0: None,
            library: Arc::default(),
            session_id: String::new(),
            session_started: 0.0,
            cover: String::new(),
            open: None,
            countdown: None,
            show_history: false,
            stopping: false,
            last_progress: 0.0,
            stopped_early: false,
            manual: false,
            notes: Vec::new(),
            soulseek_down: false,
            hover_started: None,
            scrub: None,
        }
    }
}

/// What the tab strip shows for one tab.
#[derive(Clone, Debug, PartialEq)]
pub struct TabInfo {
    pub label: String,
    pub phase: Phase,
    /// 0 to 1 while it runs.
    pub progress: f32,
}

pub struct State {
    /// Every tab; the open one (`tab`) is only a placeholder: its fields are in this struct.
    pub tabs: Vec<Tab>,
    pub tab: usize,
    /// The engine every tab's run uses (one Soulseek session), once one has run.
    pub engine: Option<Arc<backend::Engine>>,
    /// The last id given to a run or an engine.
    ids: u64,
    pub link: TextEdit,
    pub phase: Phase,
    /// The backend's phase: spotify, soulseek, youtube, convert, done.
    pub stage: String,
    pub name: String,
    pub kind: String,
    pub tracks: Vec<Track>,
    pub convert: (usize, usize),
    pub files: usize,
    pub out_dir: String,
    pub error: String,
    pub notice: String,
    pub last_log: String,
    /// Download speed in bytes per second, smoothed.
    pub speed: f32,
    speed_bytes: u64,
    speed_t: f64,
    pub settings: Settings,
    pub settings_open: bool,
    pub dir_edit: TextEdit,
    /// The Soulseek account the runs use.
    pub account: Account,
    /// The sign-in form (first launch, and in the settings).
    pub login_user: TextEdit,
    pub login_pass: TextEdit,
    pub login_busy: bool,
    pub login_error: String,
    /// The sign-in window that opens at launch while there is no account.
    pub login_open: bool,
    /// The engine could not sign in to Soulseek (wrong password, account in use elsewhere).
    pub login_problem: bool,
    pub lang: Lang,
    pub run: Option<Run>,
    /// The id of this tab's current run (its engine `job`); events of other ids are not its.
    pub run_id: u64,
    /// The tracks the next run downloads (a follow-up run), or None for the first run.
    next_only: Option<Vec<usize>>,
    pub running_link: String,
    pub handle: Option<AppHandle<State>>,
    /// `SPOTSEEK_DEMO=1`: runs play a scripted download on the app's clock, with no backend.
    pub demo: bool,
    demo_t0: Option<f64>,
    pub now: f64,
    /// Saves history, logs and covers (not in tests, headless scenes or the demo).
    persist: bool,
    pub history: History,
    library: Arc<Mutex<Option<Library>>>,
    /// The session of the current link.
    pub session_id: String,
    pub session_started: f64,
    /// The cover image file of the current link, once the backend has saved it.
    pub cover: String,
    /// The open details panel: "c{i}" for a track of this run, "h{session}:{j}" in the history.
    pub open: Option<String>,
    /// The "you already have these" countdown: (seconds left, length). It only runs while
    /// the window has focus, so nothing is skipped while the user cannot see the question.
    pub countdown: Option<(f64, f64)>,
    last_tick: f64,
    /// The window has keyboard focus.
    pub focused: bool,
    /// A drag on a waveform: (path, 0..1).
    pub scrub: Option<(String, f32)>,
    /// The history screen, opened from the paste screen.
    pub show_history: bool,
    /// STOP was pressed: the backend converts the finished tracks, then the poster shows.
    pub stopping: bool,
    /// The app clock at the last download progress (a track event or bytes).
    pub last_progress: f64,
    /// The last run was stopped early: tracks it did not get are "not downloaded", not missing.
    pub stopped_early: bool,
    /// The run waits for the user to pick a file for each track.
    pub manual: bool,
    /// The log: what happens, in plain words (and the technical lines).
    pub notes: Vec<Note>,
    /// The log panel is open, and it shows the technical lines too.
    pub log_open: bool,
    pub log_tech: bool,
    /// The Soulseek session dropped and the engine signs in again.
    pub soulseek_down: bool,
    /// The track that plays because the mouse is over it.
    pub hover_started: Option<usize>,
    /// A newer Spotseek: the banner on the paste screen, and the update as it goes.
    pub update: Up,
    pub media: Media,
}

impl State {
    pub fn new(handle: Option<AppHandle<State>>) -> State {
        // Tests never read the settings of the person who runs them.
        let settings = if cfg!(test) { Settings::default() } else { Settings::load() };
        let demo = std::env::var_os("SPOTSEEK_DEMO").is_some_and(|v| v != "0");
        let persist = handle.is_some() && !demo;
        let wake = handle.clone();
        // Tests and headless scenes (no handle) never open a sound output.
        let media = if handle.is_some() {
            Media::new(move || {
                if let Some(h) = &wake {
                    h.update(|_, _| {});
                }
            })
        } else {
            Media::silent()
        };
        State {
            tabs: vec![Tab::default()],
            tab: 0,
            engine: None,
            ids: 0,
            link: TextEdit::new(""),
            phase: Phase::Idle,
            stage: String::new(),
            name: String::new(),
            kind: String::new(),
            tracks: Vec::new(),
            convert: (0, 0),
            files: 0,
            out_dir: String::new(),
            error: String::new(),
            notice: String::new(),
            last_log: String::new(),
            speed: 0.0,
            speed_bytes: 0,
            speed_t: 0.0,
            dir_edit: TextEdit::new(&settings.dir),
            account: if persist { account::current(&settings.user) } else { Account::None },
            login_user: TextEdit::new(&settings.user),
            login_pass: TextEdit::new("").secure(),
            login_busy: false,
            login_error: String::new(),
            login_open: false,
            login_problem: false,
            lang: Lang::from_setting(&settings.lang),
            settings,
            settings_open: false,
            run: None,
            run_id: 0,
            next_only: None,
            running_link: String::new(),
            handle,
            demo,
            demo_t0: None,
            now: 0.0,
            persist,
            history: if persist { History::load() } else { History::default() },
            library: Arc::default(),
            session_id: String::new(),
            session_started: 0.0,
            cover: String::new(),
            open: None,
            countdown: None,
            last_tick: 0.0,
            focused: true,
            scrub: None,
            show_history: false,
            stopping: false,
            last_progress: 0.0,
            stopped_early: false,
            manual: false,
            notes: Vec::new(),
            log_open: false,
            log_tech: false,
            soulseek_down: false,
            hover_started: None,
            update: Up::None,
            media,
        }
    }

    /// Cmd-V with no field focused: the text becomes the link.
    pub fn paste(&mut self, text: &str) {
        if self.phase == Phase::Idle && !self.settings_open {
            self.show_history = false;
            self.link.replace_text(text.trim());
            self.notice.clear();
        }
    }

    pub fn close_settings(&mut self) {
        let dir = self.dir_edit.text.trim();
        if !dir.is_empty() {
            self.settings.dir = dir.to_string();
        }
        if self.persist {
            self.settings.save();
        }
        self.settings_open = false;
    }

    /// The pasted text as a Spotify or SoundCloud link, or None.
    pub fn link_url(&self) -> Option<String> {
        spotify_link(&self.link.text).or_else(|| soundcloud_link(&self.link.text))
    }

    /// Starts a session for the pasted link.
    pub fn start(&mut self) {
        if self.phase == Phase::Running {
            return;
        }
        let Some(url) = self.link_url() else {
            self.notice = self.lang.t("THAT IS NOT A SPOTIFY OR SOUNDCLOUD LINK", "ESO NO ES UN LINK DE SPOTIFY NI DE SOUNDCLOUD").into();
            return;
        };
        self.reset_run();
        self.show_history = false;
        self.phase = Phase::Running;
        self.stage = if url.contains("soundcloud.com") { "soundcloud" } else { "spotify" }.into();
        self.running_link = url;
        self.session_started = history::now();
        self.session_id = session_id();
        if self.demo {
            self.run_id = self.fresh_id();
            self.demo_t0 = Some(self.now);
            return;
        }
        // Read the folder while Spotify answers; the collection event needs it.
        if self.persist {
            let (lib, hist, dir) = (self.library.clone(), self.history.clone(), self.settings.dir.clone());
            *lib.lock().unwrap() = None;
            std::thread::spawn(move || {
                let l = Library::scan(&hist, &dir);
                *lib.lock().unwrap() = Some(l);
            });
        }
        self.next_only = None;
        self.launch();
    }

    /// A number no run or engine had before.
    fn fresh_id(&mut self) -> u64 {
        self.ids += 1;
        self.ids
    }

    /// The engine for the current account: the running one, or a new one. A run that is
    /// still going on an engine of another account keeps it until it ends.
    fn engine_for(&mut self, req: &backend::EngineRequest) -> Result<Arc<backend::Engine>, String> {
        if let Some(e) = self.engine.as_ref().filter(|e| e.serves(req)) {
            return Ok(e.clone());
        }
        let Some(handle) = self.handle.clone() else { return Err("no window".into()) };
        let id = self.fresh_id();
        let (events, exits) = (handle.clone(), handle);
        let e = backend::Engine::start(
            id,
            req,
            move |e| events.update(move |s, _| s.engine_event(id, e)),
            move |msg| exits.update(move |s, _| s.engine_exited(id, msg)),
        )?;
        self.engine = Some(e.clone());
        Ok(e)
    }

    /// Starts a backend run for the current link (on the engine every tab shares).
    fn launch(&mut self) {
        self.run_id = self.fresh_id();
        self.countdown = None;
        if self.handle.is_none() {
            return;
        }
        let stamp = session_id();
        let ereq = backend::EngineRequest {
            user: match &self.account {
                Account::App(u, _) => u.clone(),
                _ => String::new(),
            },
            pass: match &self.account {
                Account::App(_, p) => p.clone(),
                _ => String::new(),
            },
            skip_soulseek: self.account == Account::None,
            log: self.persist.then(|| config_dir().join("logs").join(format!("{stamp}.log"))),
        };
        let req = backend::Request {
            url: self.running_link.clone(),
            dir: self.settings.dir.clone(),
            format: self.settings.format.clone(),
            bitrate: self.settings.bitrate.clone(),
            fallback: self.settings.fallback,
            ask: true,
            manual: self.settings.manual,
            cover_out: if self.persist && self.cover.is_empty() { config_dir().join("covers").join(format!("{}.jpg", self.session_id)).to_string_lossy().into_owned() } else { String::new() },
        };
        match self.engine_for(&ereq) {
            Ok(engine) => {
                self.stage = if self.running_link.contains("soundcloud.com") { "soundcloud" } else { "spotify" }.into();
                self.run = Some(Run::start(&engine, self.run_id, &req));
            }
            Err(e) => {
                self.phase = Phase::Failed;
                self.error = e;
            }
        }
    }

    // ---------------------------------------------------------------- tabs

    /// Runs `f` with tab `k`'s fields in place (the open tab's are already there).
    pub fn with_tab<R>(&mut self, k: usize, f: impl FnOnce(&mut State) -> R) -> R {
        if k == self.tab || k >= self.tabs.len() {
            return f(self);
        }
        let mut t = std::mem::take(&mut self.tabs[k]);
        t.swap_with(self);
        let out = f(self);
        t.swap_with(self);
        self.tabs[k] = t;
        out
    }

    /// The tab whose current run is `job`.
    fn tab_of_run(&self, job: u64) -> Option<usize> {
        if job == 0 {
            return None;
        }
        if self.run_id == job {
            return Some(self.tab);
        }
        self.tabs.iter().enumerate().find(|(k, t)| *k != self.tab && t.run_id == job).map(|(k, _)| k)
    }

    /// The tabs with a run on engine `id`.
    fn tabs_on_engine(&self, id: u64) -> Vec<usize> {
        (0..self.tabs.len())
            .filter(|&k| {
                let run = if k == self.tab { &self.run } else { &self.tabs[k].run };
                run.as_ref().is_some_and(|r| r.engine.id == id)
            })
            .collect()
    }

    /// An event of the engine: to the tab of its run, or (job 0: the Soulseek session) to
    /// every tab that runs on it.
    fn engine_event(&mut self, engine: u64, e: Event) {
        if e.job == 0 {
            for k in self.tabs_on_engine(engine) {
                let e = e.clone();
                self.with_tab(k, |s| s.apply(e));
            }
        } else if let Some(k) = self.tab_of_run(e.job) {
            self.with_tab(k, |s| s.apply(e));
        }
    }

    /// The engine process ended: every run on it ends too.
    fn engine_exited(&mut self, engine: u64, msg: Option<String>) {
        if self.engine.as_ref().is_some_and(|e| e.id == engine) {
            self.engine = None;
        }
        for k in self.tabs_on_engine(engine) {
            let msg = msg.clone().or_else(|| Some("the backend stopped".into()));
            self.with_tab(k, |s| s.exited(msg));
        }
    }

    /// Opens tab `k`.
    pub fn switch_tab(&mut self, k: usize) {
        if k == self.tab || k >= self.tabs.len() {
            return;
        }
        self.stop_hover();
        // The open tab's fields go to its slot; tab k's come here.
        let cur = self.tab;
        let mut slot = std::mem::take(&mut self.tabs[cur]);
        slot.swap_with(self);
        self.tabs[cur] = slot;
        self.tab = k;
        let mut t = std::mem::take(&mut self.tabs[k]);
        t.swap_with(self);
        self.tabs[k] = t;
        // Its speed counts from now (bytes kept coming while it was hidden).
        self.speed_t = self.now;
        self.speed_bytes = 0;
    }

    /// A new tab with an empty link, opened.
    pub fn new_tab(&mut self) {
        self.tabs.push(Tab::default());
        self.switch_tab(self.tabs.len() - 1);
        self.link.focused = true;
    }

    /// Closes tab `k`; a run in it stops (what it finished stays in the history).
    pub fn close_tab(&mut self, k: usize) {
        if k >= self.tabs.len() {
            return;
        }
        self.with_tab(k, |s| {
            s.demo_t0 = None;
            if let Some(mut r) = s.run.take() {
                r.stop();
            }
            if s.phase == Phase::Running {
                s.save_session();
            }
        });
        if self.tabs.len() == 1 {
            // The last tab: it becomes an empty one.
            self.run_id = self.fresh_id();
            self.reset_run();
            self.phase = Phase::Idle;
            self.show_history = false;
            self.link.replace_text("");
            self.link.focused = true;
            return;
        }
        if k == self.tab {
            // Open the tab next to it first.
            let next = if k + 1 < self.tabs.len() { k + 1 } else { k - 1 };
            self.switch_tab(next);
        }
        self.tabs.remove(k);
        if self.tab > k {
            self.tab -= 1;
        }
    }

    /// The tab strip: every tab's name and how far it is.
    pub fn tab_infos(&self) -> Vec<TabInfo> {
        (0..self.tabs.len())
            .map(|k| {
                let (phase, name, tracks, link, running) = if k == self.tab {
                    (self.phase, &self.name, &self.tracks, &self.running_link, &self.link.text)
                } else {
                    let t = &self.tabs[k];
                    (t.phase, &t.name, &t.tracks, &t.running_link, &t.link.text)
                };
                let label = if !name.is_empty() {
                    name.to_uppercase()
                } else if phase != Phase::Idle && !link.is_empty() {
                    self.lang.t("READING…", "LEYENDO…").into()
                } else if !running.trim().is_empty() {
                    self.lang.t("NEW LINK", "LINK NUEVO").into()
                } else {
                    self.lang.t("NEW TAB", "PESTAÑA NUEVA").into()
                };
                let n = tracks.iter().filter(|t| t.in_run || t.state == TState::Done || t.state == TState::Missing).count();
                let progress = if n == 0 { 0.0 } else { tracks.iter().map(|t| if t.in_run { t.progress() } else if matches!(t.state, TState::Done | TState::Missing) { 1.0 } else { 0.0 }).sum::<f32>() / n as f32 };
                TabInfo { label, phase, progress }
            })
            .collect()
    }

    /// STOP: the downloads end, and the finished tracks are still converted and kept. A second
    /// STOP while that runs stops at once.
    pub fn stop(&mut self) {
        if !self.demo && !self.stopping {
            // Unanswered questions are skipped, and nothing more is queued.
            for t in &mut self.tracks {
                if matches!(t.state, TState::Have | TState::Queued) {
                    t.state = TState::Skipped;
                }
            }
            self.countdown = None;
            if let Some(r) = self.run.as_mut() {
                self.stopping = true;
                r.finish();
                return;
            }
            if self.phase == Phase::Running && !self.tracks.is_empty() {
                self.finish();
                return;
            }
        }
        self.stop_now();
    }

    /// Stops at once: the files so far stay in the work folder.
    pub fn stop_now(&mut self) {
        self.stopping = false;
        self.demo_t0 = None;
        if let Some(mut r) = self.run.take() {
            r.stop();
        }
        // Events of the stopped run are ignored from here on.
        self.run_id = self.fresh_id();
        self.countdown = None;
        self.save_session();
        self.phase = Phase::Idle;
        self.notice = self.lang.t("STOPPED. FILES SO FAR ARE IN THE WORK FOLDER.", "FRENADO. LO QUE SE DESCARGÓ ESTÁ EN LA CARPETA DE TRABAJO.").into();
    }

    /// Back to the paste screen for a new link.
    pub fn new_link(&mut self) {
        self.demo_t0 = None;
        if let Some(mut r) = self.run.take() {
            r.stop();
        }
        if self.phase == Phase::Running {
            self.save_session();
        }
        self.run_id = self.fresh_id();
        self.reset_run();
        self.phase = Phase::Idle;
        self.show_history = false;
        self.link.replace_text("");
        self.link.focused = true;
    }

    fn reset_run(&mut self) {
        self.stage.clear();
        self.name.clear();
        self.kind.clear();
        self.tracks.clear();
        self.convert = (0, 0);
        self.files = 0;
        self.error.clear();
        self.notice.clear();
        self.last_log.clear();
        self.speed = 0.0;
        self.speed_bytes = 0;
        self.cover.clear();
        self.countdown = None;
        self.next_only = None;
        self.open = None;
        self.out_dir.clear();
        self.stopping = false;
        self.login_problem = false;
        self.stopped_early = false;
        self.manual = false;
        self.notes.clear();
        self.soulseek_down = false;
    }

    pub fn apply(&mut self, e: Event) {
        match e.ev.as_str() {
            "phase" => {
                self.stage = e.phase;
                self.last_progress = self.now;
            }
            "collection" => self.collection(e),
            "cover" => self.cover = e.path,
            "track" => {
                self.last_progress = self.now;
                let Some(t) = e.i.and_then(|i| self.tracks.get_mut(i)) else { return };
                let Some(st) = TState::from(&e.state) else { return };
                if st == TState::Downloading && (t.state != TState::Downloading || e.path != t.part) {
                    t.bytes = 0;
                }
                if e.cand.is_some() {
                    t.cand = e.cand;
                }
                if st == TState::Downloading {
                    t.part = e.path.clone();
                    t.format = e.format.clone();
                    let secs = t.cands.iter().find(|c| Some(c.id) == t.cand).map(|c| c.duration).filter(|d| *d > 0).unwrap_or(t.duration) as f64;
                    self.media.set_growing(&t.part, 0, e.total, secs);
                }
                if st == TState::Searching && t.state == TState::Downloading {
                    // That file failed; it goes on with another.
                    t.cand = None;
                    t.part.clear();
                }
                if matches!(st, TState::Downloading | TState::YtDownload) && t.started == 0.0 {
                    t.started = history::now();
                }
                t.state = st;
                if !e.peer.is_empty() {
                    t.peer = e.peer;
                }
                if e.total > 0 {
                    t.total = e.total;
                }
                if e.bitrate > 0 {
                    t.bitrate = e.bitrate;
                }
                if !e.url.is_empty() {
                    t.url = e.url;
                }
                if e.source == "youtube" {
                    t.youtube = true;
                    t.soundcloud = false;
                }
                if e.source == "soundcloud" {
                    t.soundcloud = true;
                }
                if st == TState::Done {
                    t.finished = history::now();
                    if !t.part.is_empty() && t.total > 0 {
                        let secs = self.media.growing(&t.part).map_or(t.duration as f64, |g| g.secs);
                        self.media.set_growing(&t.part, t.total, t.total, secs);
                    }
                    if t.total > 0 && !t.streamed() {
                        // Progress comes a few times a second; the end of a file can
                        // arrive only with "done".
                        if t.total > t.bytes {
                            self.speed_bytes += t.total - t.bytes;
                        }
                        t.bytes = t.total;
                    }
                }
            }
            "bytes" => {
                self.last_progress = self.now;
                let Some(t) = e.i.and_then(|i| self.tracks.get_mut(i)) else { return };
                if e.bytes > t.bytes {
                    self.speed_bytes += e.bytes - t.bytes;
                }
                t.bytes = e.bytes;
                if e.total > 0 {
                    t.total = e.total;
                }
                if !t.part.is_empty() {
                    let secs = self.media.growing(&t.part).map_or(t.duration as f64, |g| g.secs);
                    self.media.set_growing(&t.part, t.bytes, t.total, secs);
                }
            }
            "candidates" => {
                self.last_progress = self.now;
                let Some(t) = e.i.and_then(|i| self.tracks.get_mut(i)) else { return };
                t.cands = e.cands.unwrap_or_default();
                t.settled = e.settled;
            }
            "file" => {
                if let Some(t) = e.i.and_then(|i| self.tracks.get_mut(i)) {
                    // A preview that plays goes on with the mp3.
                    self.media.swap(&t.part, &e.path);
                    t.path = e.path;
                    if e.bitrate > 0 {
                        t.bitrate = e.bitrate;
                    }
                }
            }
            "convert" => self.convert = (e.done, e.count),
            "log" => {
                self.push_note(Level::Tech, String::new(), e.line.clone(), None);
                self.last_log = e.line;
            }
            "note" => self.note(e),
            "login" => self.login_problem = true,
            "result" => {
                self.run = None;
                self.stopped_early |= e.stopped;
                self.files += e.files;
                if !e.dir.is_empty() {
                    self.out_dir = e.dir;
                }
                self.run_ended();
            }
            "error" => {
                self.run = None;
                if e.message == "stopped" {
                    self.phase = Phase::Idle;
                } else {
                    self.phase = Phase::Failed;
                    self.error = e.message;
                }
            }
            _ => {}
        }
    }

    /// The track list: on the first run, find the tracks the user already has and tell the
    /// backend to leave them out; on a follow-up run, ask for the queued tracks only.
    fn collection(&mut self, e: Event) {
        let only: Vec<usize> = if let Some(only) = self.next_only.take() {
            if e.tracks.len() != self.tracks.len() {
                self.error = "the track list changed; paste the link again".into();
                if let Some(r) = self.run.as_mut() {
                    r.answer(&[]);
                }
                return;
            }
            self.manual = e.manual;
            only
        } else {
            self.name = e.name;
            self.kind = e.kind;
            self.manual = e.manual;
            self.tracks = e
                .tracks
                .iter()
                .map(|t| Track { album: t.album.clone(), id: t.id.clone(), duration: t.duration, ..Track::new(&t.artist, &t.title) })
                .collect();
            let lib = self.library_now();
            let mut only = Vec::new();
            for (i, t) in self.tracks.iter_mut().enumerate() {
                match lib.as_ref().and_then(|l| l.find(&t.id, &t.artist, &t.title)) {
                    Some(p) => {
                        t.have = p.to_string();
                        t.state = TState::Have;
                    }
                    None => only.push(i),
                }
            }
            only
        };
        for &i in &only {
            if let Some(t) = self.tracks.get_mut(i) {
                t.in_run = true;
                t.state = TState::Pending;
                t.bytes = 0;
            }
        }
        if let Some(r) = self.run.as_mut() {
            r.answer(&only);
        }
    }

    /// A note from the engine: words it, and keeps the state it tells about.
    fn note(&mut self, e: Event) {
        let title = e.i.and_then(|i| self.tracks.get(i)).map(|t| format!("{} – {}", t.artist, t.title)).unwrap_or_default();
        let text = notes::word(self.lang, &e.code, &title, &e.peer, &e.message, e.count, e.secs);
        match e.code.as_str() {
            "offline" | "reconnecting" => self.soulseek_down = true,
            "reconnected" | "signed_in" => self.soulseek_down = false,
            "login_failed" => {
                self.soulseek_down = false;
                self.login_problem = true;
            }
            _ => {}
        }
        if !e.line.is_empty() {
            self.last_log = e.line.clone();
        }
        self.push_note(Level::from(&e.level), text, e.line, e.i);
    }

    fn push_note(&mut self, level: Level, text: String, tech: String, i: Option<usize>) {
        if text.is_empty() && tech.is_empty() {
            return;
        }
        self.notes.push(Note { at: history::now(), level: if text.is_empty() { Level::Tech } else { level }, text, tech, i });
        if self.notes.len() > notes::MAX_NOTES {
            let n = self.notes.len() - notes::MAX_NOTES;
            self.notes.drain(..n);
        }
    }

    /// The newest note for people, for the footer.
    pub fn latest_note(&self) -> Option<&Note> {
        self.notes.iter().rev().find(|n| n.level != Level::Tech)
    }

    fn library_now(&self) -> Option<Library> {
        let ready = self.library.lock().unwrap().take();
        if ready.is_some() || !self.persist {
            return ready;
        }
        Some(Library::scan(&self.history, &self.settings.dir))
    }

    /// A backend run has finished: what it could not find is missing. Then start the queued
    /// tracks, or wait for the open questions, or end the session.
    fn run_ended(&mut self) {
        for t in &mut self.tracks {
            if t.in_run {
                t.in_run = false;
                if t.state != TState::Done {
                    t.state = TState::Missing;
                }
            }
        }
        self.speed = 0.0;
        self.next_step();
    }

    fn next_step(&mut self) {
        if self.phase != Phase::Running || self.run.is_some() || self.demo_t0.is_some() {
            return;
        }
        if self.stopping {
            self.stopping = false;
            return self.finish();
        }
        let queued: Vec<usize> = self.tracks.iter().enumerate().filter(|(_, t)| t.state == TState::Queued).map(|(i, _)| i).collect();
        if !queued.is_empty() {
            for &i in &queued {
                self.tracks[i].state = TState::Pending;
            }
            if self.demo {
                for &i in &queued {
                    let t = &mut self.tracks[i];
                    t.state = TState::Done;
                    t.path = format!("demo://{i}");
                }
                return self.next_step();
            }
            self.next_only = Some(queued);
            self.launch();
            return;
        }
        if self.asking() > 0 {
            if self.countdown.is_none() {
                let all = self.tracks.iter().all(|t| matches!(t.state, TState::Have | TState::Skipped));
                let len = if all { ASK_ALL_SECS } else { ASK_SECS };
                self.countdown = Some((len, len));
            }
            return;
        }
        self.finish();
    }

    /// The session is over: show the poster, save it to the history.
    fn finish(&mut self) {
        self.countdown = None;
        self.phase = Phase::Done;
        self.stage = "done".into();
        if self.out_dir.is_empty() {
            self.out_dir = self.settings.dir.clone();
        }
        // A downloaded track whose file the backend did not name: look for it by its tags.
        if self.persist && self.tracks.iter().any(|t| t.state == TState::Done && t.path.is_empty()) {
            let lib = Library::scan(&History::default(), &self.out_dir);
            for t in &mut self.tracks {
                if t.state == TState::Done && t.path.is_empty() {
                    if let Some(p) = lib.find("", &t.artist, &t.title) {
                        t.path = p.to_string();
                    }
                }
            }
        }
        self.save_session();
    }

    fn save_session(&mut self) {
        if !self.persist || self.session_id.is_empty() {
            return;
        }
        // A done track without a file (a run stopped at once) is still in the work folder: leave
        // it out, or the history would show it as moved.
        let tracks: Vec<SavedTrack> = self
            .tracks
            .iter()
            .filter(|t| (t.state == TState::Done && !t.path.is_empty()) || (t.state == TState::Missing && !self.stopped_early))
            .map(Track::saved)
            .collect();
        if !tracks.iter().any(|t| t.state == "done") {
            return;
        }
        self.history.put(Session {
            id: self.session_id.clone(),
            link: self.running_link.clone(),
            name: self.name.clone(),
            kind: self.kind.clone(),
            cover: self.cover.clone(),
            dir: if self.out_dir.is_empty() { self.settings.dir.clone() } else { self.out_dir.clone() },
            started: self.session_started,
            finished: history::now(),
            tracks,
        });
        self.history.save();
    }

    fn exited(&mut self, msg: Option<String>) {
        self.run = None;
        if self.phase == Phase::Running && msg.is_some() {
            self.phase = Phase::Failed;
            self.error = msg.unwrap_or_default();
        }
    }

    // ---------------------------------------------------------------- account

    /// Opens the sign-in window at launch when there is no account.
    // ---------------------------------------------------------------- updates

    /// At launch: asks for a newer version (not in tests, headless scenes or the demo).
    pub fn check_update(&self) {
        if !self.persist || std::env::var_os("SPOTSEEK_NO_UPDATE_CHECK").is_some() {
            return;
        }
        let Some(h) = self.handle.clone() else { return };
        std::thread::spawn(move || {
            if let Some(l) = update::check() {
                h.update(move |s, _| s.update_found(l));
            }
        });
    }

    pub fn update_found(&mut self, l: update::Line) {
        if l.newer && l.version != self.settings.skip_update && self.update == Up::None {
            self.update = Up::Ready { version: l.version, page: l.page, kind: l.kind };
        }
    }

    /// LATER: this version is not offered again (a newer one is).
    pub fn update_later(&mut self) {
        self.settings.skip_update = self.update.version().to_string();
        if self.persist {
            self.settings.save();
        }
        self.update = Up::None;
    }

    /// UPDATE: downloads and installs it, then Spotseek starts again. Never during a download;
    /// a copy that cannot replace itself opens the download page.
    pub fn update_now(&mut self, tx: &mut rune::Tx) {
        let (version, page) = (self.update.version().to_string(), self.update.page());
        if self.update.busy() || version.is_empty() || self.phase == Phase::Running {
            return;
        }
        if matches!(&self.update, Up::Ready { kind, .. } if kind == "manual") {
            let _ = tx.open_url(&page);
            return;
        }
        let Some(h) = self.handle.clone() else { return };
        self.update = Up::Downloading { version, page, frac: 0.0 };
        std::thread::spawn(move || {
            update::install(|l| h.update(move |s, tx| s.update_line(l, tx)));
        });
    }

    pub fn update_line(&mut self, l: update::Line, tx: &mut rune::Tx) {
        let (version, page) = (self.update.version().to_string(), self.update.page());
        match l.state.as_str() {
            "downloading" => {
                let frac = if l.total > 0 { (l.bytes as f64 / l.total as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
                self.update = Up::Downloading { version, page, frac };
            }
            "installing" => self.update = Up::Installing { version },
            "ready" => {
                self.media.stop();
                match update::launch(&l.run, &l.args) {
                    Ok(()) => tx.quit(),
                    Err(e) => self.update = Up::Failed { version, page, message: e.to_string() },
                }
            }
            "failed" => self.update = Up::Failed { version, page, message: l.message },
            _ => {}
        }
    }

    pub fn ask_login_at_launch(&mut self) {
        if self.persist && self.account == Account::None {
            self.login_open = true;
            self.login_user.focused = true;
        }
    }

    /// Signs in (or creates the account) with the name and password in the form.
    pub fn sign_in(&mut self) {
        let user = self.login_user.text.trim().to_string();
        let pass = self.login_pass.text.clone();
        if user.is_empty() || pass.is_empty() {
            self.login_error = self.lang.t("Type a name and a password.", "Escribí un nombre y una contraseña.").into();
            return;
        }
        if self.login_busy {
            return;
        }
        self.login_busy = true;
        self.login_error.clear();
        let Some(h) = self.handle.clone() else {
            self.login_busy = false;
            return;
        };
        std::thread::spawn(move || {
            let r = account::check(&user, &pass);
            h.update(move |s, _| s.signed_in(r, user, pass));
        });
    }

    pub fn signed_in(&mut self, r: Check, user: String, pass: String) {
        self.login_busy = false;
        let l = self.lang;
        match r {
            Check::Ok => {
                if let Err(e) = account::save(&user, &pass) {
                    // It works for this session; it is asked again at the next launch.
                    self.login_error = format!("{}: {e}", l.t("Signed in, but the password could not be saved", "Entraste, pero no se pudo guardar la contraseña"));
                } else {
                    self.login_error.clear();
                }
                self.settings.user = user.clone();
                if self.persist {
                    self.settings.save();
                }
                self.account = Account::App(user, pass);
                self.login_pass.replace_text("");
                self.login_open = false;
                self.login_problem = false;
            }
            Check::WrongPassword => self.login_error = l.t("That name is taken, or the password is wrong.", "Ese nombre ya existe o la contraseña es incorrecta.").into(),
            Check::BadName => self.login_error = l.t("Soulseek does not accept that name: up to 30 letters, numbers or symbols, no accents.", "Soulseek no acepta ese nombre: hasta 30 letras, números o símbolos, sin tildes.").into(),
            Check::ServerFull => self.login_error = l.t("The Soulseek server is full. Try again later.", "El servidor de Soulseek está lleno. Probá más tarde.").into(),
            Check::Error(e) => self.login_error = format!("{} {e}", l.t("Cannot reach Soulseek:", "No se puede conectar con Soulseek:")),
        }
    }

    pub fn sign_out(&mut self) {
        if let Account::App(u, _) = &self.account {
            account::forget(u);
        }
        self.settings.user.clear();
        if self.persist {
            self.settings.save();
        }
        self.account = if self.persist { account::current("") } else { Account::None };
        self.login_user.replace_text("");
    }

    pub fn set_lang(&mut self, setting: &str) {
        self.settings.lang = setting.to_string();
        self.lang = Lang::from_setting(setting);
    }

    // ---------------------------------------------------------------- questions

    /// Tracks that wait for "download again" or "skip".
    pub fn asking(&self) -> usize {
        self.count(TState::Have)
    }

    /// The answer for track `i`: download it again, or skip it.
    pub fn decide(&mut self, i: usize, again: bool) {
        let Some(t) = self.tracks.get_mut(i) else { return };
        if t.state != TState::Have {
            return;
        }
        t.state = if again { TState::Queued } else { TState::Skipped };
        self.after_answer(again);
    }

    pub fn decide_all(&mut self, again: bool) {
        for t in &mut self.tracks {
            if t.state == TState::Have {
                t.state = if again { TState::Queued } else { TState::Skipped };
            }
        }
        self.after_answer(again);
    }

    fn after_answer(&mut self, again: bool) {
        if again || self.asking() == 0 {
            self.countdown = None;
        }
        self.next_step();
    }

    /// Seconds Soulseek has sent nothing while tracks still wait to be searched, or None. Soulseek
    /// allows only 34 searches every 220 s (more gets the account banned for 30 minutes), so long
    /// lists pause; the pause is seen from the silence.
    pub fn soulseek_wait(&self) -> Option<f64> {
        if self.phase != Phase::Running || self.stopping || self.stage != "soulseek" || self.downloading() {
            return None;
        }
        if !self.tracks.iter().any(|t| t.in_run && matches!(t.state, TState::Pending | TState::Searching) && !t.choosing(self.manual)) {
            return None;
        }
        let quiet = self.now - self.last_progress;
        (quiet >= 20.0).then_some(quiet)
    }

    /// Seconds left on the countdown.
    pub fn countdown_left(&self) -> Option<f64> {
        self.countdown.map(|(left, _)| left.max(0.0))
    }

    /// The window got or lost focus. Back in front, the user gets a few seconds at least.
    pub fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            // A hover play stops when the window goes to the back.
            self.stop_hover();
        }
        if let (true, Some((left, _))) = (focused, self.countdown.as_mut()) {
            *left = left.max(4.0);
        }
    }

    // ---------------------------------------------------------------- picks

    /// Downloads candidate `cand` for track `i`, or (None) takes it from YouTube Music.
    pub fn pick(&mut self, i: usize, cand: Option<u32>) {
        let Some(t) = self.tracks.get_mut(i) else { return };
        if !t.in_run || matches!(t.state, TState::Done | TState::Missing | TState::Failed | TState::YtSearch | TState::YtDownload) {
            return;
        }
        if cand.is_some() && cand == t.cand {
            return;
        }
        if let Some(r) = self.run.as_mut() {
            r.pick(i, cand);
            // Shown at once; the backend's events follow.
            t.cand = cand;
            if cand.is_none() {
                t.state = TState::Failed;
            }
        }
    }

    /// Manual mode: the best file for every track that waits for a pick. A track whose files
    /// all failed goes to YouTube Music (when the fallback is on), so nothing is left waiting.
    pub fn pick_best_for_all(&mut self) {
        let fallback = self.settings.fallback;
        let picks: Vec<(usize, Option<u32>)> = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.choosing(self.manual))
            .filter_map(|(i, t)| match t.cands.iter().find(|c| !c.failed) {
                Some(c) => Some((i, Some(c.id))),
                None if fallback => Some((i, None)),
                None => None,
            })
            .collect();
        for (i, id) in picks {
            self.pick(i, id);
        }
    }

    pub fn save_settings(&mut self) {
        if self.persist {
            self.settings.save();
        }
    }

    /// The file of track `i` that can play now: the mp3, the one already in the folder, or the
    /// part of a Soulseek download that is on disk.
    pub fn playable(&self, i: usize) -> Option<String> {
        let t = self.tracks.get(i)?;
        if !t.path.is_empty() && !t.path.starts_with("demo://") {
            return Some(t.path.clone());
        }
        if !t.have.is_empty() && !t.have.starts_with("demo://") {
            return Some(t.have.clone());
        }
        if t.previewable() && matches!(t.state, TState::Downloading | TState::Done) && self.media.growing(&t.part).is_some_and(|g| g.complete() || g.playable() >= 2.0) {
            return Some(t.part.clone());
        }
        None
    }

    /// The mouse came over track `i` (`true`) or left it. With hover play on, it plays from
    /// where it was left, and pauses when the mouse leaves (not while its details are open).
    pub fn hover_track(&mut self, i: usize, on: bool) {
        if !self.settings.hover_play || self.demo {
            return;
        }
        if on {
            let Some(path) = self.playable(i) else { return };
            self.media.hover(&path, true);
            self.hover_started = Some(i);
        } else if self.hover_started == Some(i) {
            if self.open.as_deref() == Some(format!("c{i}").as_str()) {
                return;
            }
            self.stop_hover();
        }
    }

    /// Pauses what a hover started.
    pub fn stop_hover(&mut self) {
        if self.hover_started.take().is_some() {
            self.media.pause();
        }
    }

    /// PLAY: ON CLICK / ON HOVER. Turning it off pauses what a hover started; turning it on the
    /// first time says how it works.
    pub fn set_hover_play(&mut self, on: bool) {
        if self.settings.hover_play == on {
            return;
        }
        self.settings.hover_play = on;
        if on {
            let hint = self.lang.t("Move the mouse over a song to hear it. Space pauses.", "Pasá el mouse por una canción para oírla. Espacio pausa.").to_string();
            self.push_note(Level::Info, hint, String::new(), None);
        } else {
            self.stop_hover();
        }
        self.save_settings();
    }

    /// The track whose file plays (or is loaded in the player).
    pub fn playing_track(&self) -> Option<usize> {
        let st = self.media.status();
        if st.path.is_empty() {
            return None;
        }
        self.tracks.iter().position(|t| t.path == st.path || (!t.part.is_empty() && t.part == st.path) || (!t.have.is_empty() && t.have == st.path))
    }

    /// Tracks that wait for a pick.
    pub fn choosing(&self) -> usize {
        self.tracks.iter().filter(|t| t.choosing(self.manual)).count()
    }

    // ---------------------------------------------------------------- details

    /// Opens the details of `key`, or closes them when they are open.
    pub fn toggle_open(&mut self, key: String) {
        self.open = if self.open.as_deref() == Some(key.as_str()) { None } else { Some(key) };
    }

    pub fn detail_of(&self, key: &str) -> Option<Detail> {
        if let Some(i) = key.strip_prefix('c').and_then(|s| s.parse::<usize>().ok()) {
            return self.tracks.get(i).map(|t| self.detail(t));
        }
        let (sid, j) = key.strip_prefix('h')?.split_once(':')?;
        let s = self.history.sessions.iter().find(|s| s.id == sid)?;
        let t = s.tracks.get(j.parse::<usize>().ok()?)?;
        Some(Detail::from_saved(t, if s.kind == "playlist" { "" } else { &s.cover }, self.lang))
    }

    fn detail(&self, t: &Track) -> Detail {
        let pct = if t.total > 0 { t.bytes * 100 / t.total } else { 0 };
        let l = self.lang;
        let status = match t.state {
            _ if t.choosing(self.manual) => l.t("PICK A FILE BELOW", "ELEGÍ UN ARCHIVO ABAJO").into(),
            TState::Pending => l.t("WAITING", "ESPERANDO").into(),
            TState::Searching => l.t("SEARCHING SOULSEEK", "BUSCANDO EN SOULSEEK").into(),
            TState::Downloading => format!("{} {pct}%", l.t("DOWNLOADING", "DESCARGANDO")),
            TState::Done => l.t("DOWNLOADED", "DESCARGADA").into(),
            TState::Failed => l.t("NOT ON SOULSEEK", "NO ESTÁ EN SOULSEEK").into(),
            TState::YtSearch => l.t("SEARCHING YOUTUBE MUSIC", "BUSCANDO EN YOUTUBE MUSIC").into(),
            TState::YtDownload if t.soundcloud => l.t("DOWNLOADING FROM SOUNDCLOUD", "DESCARGANDO DE SOUNDCLOUD").into(),
            TState::YtDownload => l.t("DOWNLOADING FROM YOUTUBE", "DESCARGANDO DE YOUTUBE").into(),
            TState::Missing => l.t("NOT FOUND", "NO ENCONTRADA").into(),
            TState::Have => l.t("ALREADY IN YOUR FOLDER", "YA ESTÁ EN TU CARPETA").into(),
            TState::Queued => l.t("WILL DOWNLOAD AGAIN", "SE VA A DESCARGAR DE NUEVO").into(),
            TState::Skipped => l.t("ALREADY IN YOUR FOLDER · SKIPPED", "YA ESTÁ EN TU CARPETA · SALTEADA").into(),
        };
        let path = if !t.path.is_empty() {
            t.path.clone()
        } else if !t.have.is_empty() {
            t.have.clone()
        } else if t.previewable() && matches!(t.state, TState::Downloading | TState::Done) {
            // The preview: the file as it downloads.
            t.part.clone()
        } else {
            String::new()
        };
        let size = if t.total > 0 { t.total } else { std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) };
        let preview = !path.is_empty() && path == t.part;
        let finished = if t.state == TState::Downloading { history::now() } else { t.finished };
        Detail {
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration: t.duration,
            status,
            source: if t.soundcloud { "soundcloud" } else if t.youtube { "youtube" } else if t.peer.is_empty() { "" } else { "soulseek" }.into(),
            peer: t.peer.clone(),
            url: t.url.clone(),
            bitrate: t.bitrate,
            size: if t.state == TState::Downloading { t.bytes } else { size },
            path,
            started: t.started,
            finished,
            cover: if self.kind == "playlist" { String::new() } else { self.cover.clone() },
            moved: false,
            bad: t.state == TState::Missing,
            preview,
        }
    }

    // ---------------------------------------------------------------- clock

    /// Samples the download speed, runs the countdown (and moves a demo run). True when
    /// something changed.
    pub fn tick(&mut self, now: f64) -> bool {
        self.now = now;
        // A long gap (the app was asleep) counts as a short one.
        let dt = (now - self.last_tick).clamp(0.0, 0.25);
        self.last_tick = now;
        let mut changed = false;
        if let Some((left, _)) = self.countdown.as_mut() {
            changed = true;
            if self.focused {
                *left -= dt;
            }
            if *left <= 0.0 {
                self.decide_all(false);
            }
        }
        if let Some(t0) = self.demo_t0 {
            if self.phase == Phase::Running {
                self.demo_step(now - t0);
                return true;
            }
            self.demo_t0 = None;
        }
        let dt = now - self.speed_t;
        if dt < 0.5 {
            return changed;
        }
        self.speed_t = now;
        let inst = self.speed_bytes as f32 / dt as f32;
        self.speed_bytes = 0;
        let old = self.speed;
        self.speed = if inst > 0.0 { self.speed * 0.4 + inst * 0.6 } else { self.speed * 0.7 };
        if self.speed < 1000.0 {
            self.speed = 0.0;
        }
        changed || (old - self.speed).abs() > 1.0
    }

    /// The scripted demo run at `e` seconds after its start.
    fn demo_step(&mut self, e: f64) {
        let hash = |i: usize| ((i as f64 * 12.9898).sin() * 43758.5453).fract().abs();
        if e < 0.9 {
            self.stage = "spotify".into();
            self.last_log = "Reading Spotify metadata...".into();
            return;
        }
        if self.tracks.is_empty() {
            self.name = "Fiesta 2005".into();
            self.kind = "playlist".into();
            self.tracks = crate::DEMO.iter().map(|(a, t)| Track { duration: 180 + (hash(t.len()) * 90.0) as u32, ..Track::new(a, t) }).collect();
            for (i, t) in self.tracks.iter_mut().enumerate() {
                if crate::DEMO_HAVE.contains(&i) {
                    t.state = TState::Have;
                    t.have = format!("demo://{i}");
                } else {
                    t.in_run = true;
                }
            }
            self.stage = "soulseek".into();
        }
        let n = self.tracks.len();
        let mut rate = 0.0;
        let mut last_end = 0.0f64;
        let now = history::now();
        for i in 0..n {
            if !self.tracks[i].in_run {
                continue;
            }
            let search = 1.2 + i as f64 * 0.06;
            let start = 3.4 + i as f64 * 0.3;
            let dur = 1.0 + hash(i) * 1.4;
            let total = 6_000_000 + (hash(i + 7) * 4_000_000.0) as u64;
            let failed = i == 23 || i == 31;
            let yt = i == 9 || i == 23;
            let t = &mut self.tracks[i];
            t.total = total;
            let end = start + dur;
            last_end = last_end.max(end + if failed { 2.5 } else { 0.0 });
            let st = if e < search {
                TState::Pending
            } else if e < start {
                TState::Searching
            } else if failed {
                if e < start + 0.8 { TState::Failed } else if i == 31 && e > start + 1.6 { TState::Missing } else if e < start + 1.6 { TState::YtSearch } else if e < start + 2.5 { TState::YtDownload } else { TState::Done }
            } else if e < end {
                TState::Downloading
            } else {
                TState::Done
            };
            if st != t.state && matches!(st, TState::Downloading | TState::YtDownload) {
                t.started = now;
            }
            if st != t.state && st == TState::Done {
                t.finished = now;
                t.path = format!("demo://{i}");
            }
            t.state = st;
            t.youtube = yt || failed;
            t.bitrate = 320;
            t.peer = if t.youtube { "YouTube Music".into() } else { ["zwem", "xXdj_nico", "rodiganhomelab", "hfegetude", "djcotter"][i % 5].into() };
            if t.state == TState::Downloading {
                let p = ((e - start) / dur).clamp(0.0, 1.0);
                t.bytes = (total as f64 * p) as u64;
                rate += total as f64 / dur;
            } else if t.state == TState::Done {
                t.bytes = total;
            }
            if t.state == TState::Downloading || t.state == TState::Searching {
                self.last_log = format!("[{:03}] SongJob: {}: {} - {}", i + 3, if t.state == TState::Searching { "searching" } else { "downloading" }, t.artist, t.title);
            }
        }
        // Shown at a believable Soulseek speed.
        self.speed = rate as f32 / 22.0;
        if e > last_end {
            self.stage = "convert".into();
            let k = ((e - last_end) / 1.6 * n as f64) as usize;
            self.convert = (k.min(n), n);
            self.last_log = format!("Converting to {}...", self.settings.quality_label());
            if e > last_end + 1.8 {
                self.demo_t0 = None;
                self.files = self.count(TState::Done);
                self.out_dir = self.settings.dir.clone();
                self.stage = "done".into();
                self.run_ended();
            }
        }
    }

    pub fn count(&self, st: TState) -> usize {
        self.tracks.iter().filter(|t| t.state == st).count()
    }

    /// Tracks with nothing more to do in this session.
    pub fn finished(&self) -> usize {
        self.tracks.iter().filter(|t| matches!(t.state, TState::Done | TState::Missing | TState::Skipped | TState::Have)).count()
    }

    pub fn downloading(&self) -> bool {
        self.tracks.iter().any(|t| matches!(t.state, TState::Downloading | TState::YtDownload))
    }

    /// Overall progress, 0 to 1: the tracks of this run, then the conversion.
    pub fn progress(&self) -> f32 {
        if self.phase == Phase::Done {
            return 1.0;
        }
        let run: Vec<&Track> = self.tracks.iter().filter(|t| t.in_run).collect();
        if run.is_empty() {
            return if self.tracks.is_empty() { 0.0 } else { 1.0 };
        }
        let tracks = run.iter().map(|t| t.progress()).sum::<f32>() / run.len() as f32;
        let conv = if self.convert.1 > 0 { self.convert.0 as f32 / self.convert.1 as f32 } else { 0.0 };
        if self.stage == "convert" { 0.9 + 0.1 * conv } else { tracks * 0.9 }
    }

    /// The peers that send files now, newest first, without repeats.
    pub fn peers(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for t in self.tracks.iter().rev() {
            if matches!(t.state, TState::Downloading | TState::YtDownload) && !t.peer.is_empty() && !out.contains(&t.peer.as_str()) {
                out.push(&t.peer);
            }
        }
        out
    }

    /// Headline effect data: `[word, wobble, glitch, 0]`. Spanish words follow the English ones.
    pub fn headline(&self) -> [f32; 4] {
        let mut d = self.headline_en();
        if self.lang.es() {
            d[0] += gfx::WORDS_PER_LANG as f32;
        }
        d
    }

    fn headline_en(&self) -> [f32; 4] {
        match self.phase {
            Phase::Idle => {
                let has_text = !self.link.text.trim().is_empty();
                let ok = self.link_url().is_some();
                let glitch = if has_text && !ok { 0.35 } else { 0.0 };
                [gfx::W_PASTE, if ok { 1.0 } else { 0.0 }, glitch, 0.0]
            }
            Phase::Running => {
                let running = self.tracks.iter().any(|t| t.in_run || t.state == TState::Queued);
                match self.stage.as_str() {
                    "convert" if running => [gfx::W_CONVERTING, 0.4, 0.25, 0.0],
                    _ if !self.tracks.is_empty() && !running && self.asking() > 0 => [gfx::W_HAVE, 0.3, 0.1, 0.0],
                    _ if self.downloading() || self.count(TState::Done) > 0 => [gfx::W_DOWNLOADING, 0.45, 0.12, 0.0],
                    _ => [gfx::W_SEARCHING, 0.6, 0.45, 0.0],
                }
            }
            Phase::Done => [gfx::W_DONE, 0.0, 0.0, 0.0],
            Phase::Failed => [gfx::W_FAILED, 0.2, 0.6, 0.0],
        }
    }

    /// Progress bar effect data: `[fill, speed, peers sending (0 to 4), tracks done]`.
    pub fn bar(&self) -> [f32; 4] {
        let speed = (self.speed / 2_500_000.0).clamp(0.0, 1.0);
        let peers = if self.phase == Phase::Running { self.peers().len().min(4) as f32 } else { 0.0 };
        [self.progress(), speed, peers, self.count(TState::Done) as f32]
    }
}

/// A sortable id from the clock: "20261107-153012-123".
fn session_id() -> String {
    let t = history::now();
    let secs = t as i64;
    let ms = ((t - secs as f64) * 1000.0) as i64;
    // Days since 1970 to a date (UTC), without a date library.
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}-{ms:03}", rem / 3600, rem / 60 % 60, rem % 60)
}

/// Days since 1970-01-01 to (year, month, day). From Howard Hinnant's `civil_from_days`.
pub fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Accepts `open.spotify.com/{playlist,album,track}/…` (with or without the scheme or an intl
/// prefix) and `spotify:{playlist,album,track}:…`. Returns the trimmed link.
pub fn spotify_link(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return None;
    }
    if let Some(rest) = t.strip_prefix("spotify:") {
        let mut it = rest.split(':');
        let (kind, id) = (it.next()?, it.next()?);
        // The backend reads web links, so give it one.
        return (matches!(kind, "playlist" | "album" | "track") && !id.is_empty()).then(|| format!("https://open.spotify.com/{kind}/{id}"));
    }
    let rest = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://")).unwrap_or(t);
    let rest = rest.strip_prefix("open.spotify.com/")?;
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let mut kind = parts.next()?;
    if kind.starts_with("intl-") {
        kind = parts.next()?;
    }
    let id = parts.next()?.split('?').next()?;
    if matches!(kind, "playlist" | "album" | "track") && id.len() >= 10 && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(if t.starts_with("http") { t.to_string() } else { format!("https://{t}") })
    } else {
        None
    }
}

/// Accepts `soundcloud.com/<user>/<track>`, `…/sets/<name>`, `…/tracks`, `…/likes` (also
/// `m.` and `www.`, with or without the scheme) and `on.soundcloud.com/<code>` short links.
/// Returns a clean `https://` link (no `?si=` or other query).
pub fn soundcloud_link(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return None;
    }
    let rest = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://")).unwrap_or(t);
    let rest = rest.split(['?', '#']).next()?;
    let (host, path) = rest.split_once('/')?;
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match host.to_ascii_lowercase().as_str() {
        "on.soundcloud.com" if parts.len() == 1 => Some(format!("https://on.soundcloud.com/{}", parts[0])),
        // A profile alone is not enough (which of its pages?): it needs a track, a set, or its tracks/likes.
        "soundcloud.com" | "www.soundcloud.com" | "m.soundcloud.com" if parts.len() >= 2 && !["discover", "search", "you", "upload", "stream"].contains(&parts[0]) => {
            Some(format!("https://soundcloud.com/{}", parts.join("/")))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soundcloud_links() {
        assert_eq!(soundcloud_link("https://soundcloud.com/forss/flickermood?si=abc&utm_source=x").unwrap(), "https://soundcloud.com/forss/flickermood");
        assert_eq!(soundcloud_link(" m.soundcloud.com/forss/sets/soulhack ").unwrap(), "https://soundcloud.com/forss/sets/soulhack");
        assert_eq!(soundcloud_link("https://on.soundcloud.com/AbC123").unwrap(), "https://on.soundcloud.com/AbC123");
        assert!(soundcloud_link("https://soundcloud.com/forss").is_none(), "a profile alone");
        assert!(soundcloud_link("https://soundcloud.com/discover/sets/x").is_none());
        assert!(soundcloud_link("https://open.spotify.com/track/4aawyAB9vmqN3uQ7FjRGTy").is_none());
        assert!(soundcloud_link("https://soundcloud.com.example/a/b").is_none());
    }

    #[test]
    fn spotify_links() {
        assert!(spotify_link("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M").is_some());
        assert!(spotify_link("  https://open.spotify.com/intl-es/album/4aawyAB9vmqN3uQ7FjRGTy?si=x ").is_some());
        assert_eq!(spotify_link("open.spotify.com/track/4aawyAB9vmqN3uQ7FjRGTy").unwrap(), "https://open.spotify.com/track/4aawyAB9vmqN3uQ7FjRGTy");
        assert_eq!(spotify_link("spotify:track:4aawyAB9vmqN3uQ7FjRGTy").unwrap(), "https://open.spotify.com/track/4aawyAB9vmqN3uQ7FjRGTy");
        assert!(spotify_link("https://open.spotify.com/artist/4aawyAB9vmqN3uQ7FjRGTy").is_none());
        assert!(spotify_link("https://example.com/playlist/4aawyAB9vmqN3uQ7FjRGTy").is_none());
        assert!(spotify_link("hello").is_none());
    }

    #[test]
    fn tabs_keep_their_own_run_and_take_their_own_events() {
        let mut s = State::new(None);
        s.link.replace_text("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M");
        s.phase = Phase::Running;
        s.run_id = 7;
        s.new_tab();
        assert_eq!(s.tab, 1);
        assert_eq!(s.phase, Phase::Idle, "a new tab starts empty");
        assert!(s.link.text.is_empty());
        s.link.replace_text("https://soundcloud.com/forss/flickermood");
        s.run_id = 8;
        s.phase = Phase::Running;
        // The real run's events, as job 7: they go to the first tab, which is not open.
        for line in include_str!("../tests/fixtures/real-run.jsonl").lines() {
            let mut e = backend::parse(line).unwrap();
            e.job = 7;
            s.engine_event(1, e);
        }
        assert!(s.tracks.is_empty(), "the open tab got none of them");
        assert_eq!(s.tab_infos()[0].phase, Phase::Done);
        assert_eq!(s.tab_infos()[1].phase, Phase::Running);
        s.switch_tab(0);
        assert_eq!((s.phase, s.tracks.len(), s.files), (Phase::Done, 3, 3));
        assert!(s.link.text.contains("spotify"));
        assert!(s.tab_infos()[0].label.len() > 0);
        // Events of an unknown or old run go nowhere.
        let mut e = backend::parse(r#"{"ev":"phase","phase":"youtube"}"#).unwrap();
        e.job = 99;
        s.engine_event(1, e);
        s.switch_tab(1);
        assert!(s.link.text.contains("soundcloud"));
        assert_eq!(s.stage, "");
        // Closing the open tab opens its neighbour.
        s.close_tab(1);
        assert_eq!((s.tabs.len(), s.tab, s.tracks.len()), (1, 0, 3));
        // The last tab is emptied, not removed.
        s.close_tab(0);
        assert_eq!((s.tabs.len(), s.phase, s.tracks.len()), (1, Phase::Idle, 0));
    }

    #[test]
    fn a_real_run_ends_done_with_every_track_placed() {
        let mut s = State::new(None);
        s.phase = Phase::Running;
        for line in include_str!("../tests/fixtures/real-run.jsonl").lines() {
            s.apply(backend::parse(line).unwrap());
        }
        assert_eq!(s.phase, Phase::Done);
        assert_eq!(s.tracks.len(), 3);
        assert_eq!(s.count(TState::Done), 3);
        assert!(s.tracks[2].youtube, "the third track came from YouTube");
        assert_eq!(s.files, 3);
        assert_eq!(s.progress(), 1.0);
    }

    #[test]
    fn progress_grows_with_bytes() {
        let mut s = State::new(None);
        s.phase = Phase::Running;
        let lines: Vec<&str> = include_str!("../tests/fixtures/real-run.jsonl").lines().collect();
        let mut last = 0.0;
        for line in &lines[..lines.len() - 10] {
            s.apply(backend::parse(line).unwrap());
            let p = s.progress();
            assert!(p + 1e-6 >= last || s.stage == "convert", "progress went back: {last} -> {p}");
            last = p;
        }
        assert!(last > 0.5);
    }

    #[test]
    fn tracks_you_have_wait_for_an_answer_then_download_again() {
        let ev = |line: &str| backend::parse(line).unwrap();
        let mut s = State::new(None);
        // The user already has "Only Time".
        let dir = std::env::temp_dir().join(format!("spotseek-ask-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Enya - Only Time.mp3"), b"x").unwrap();
        *s.library.lock().unwrap() = Some(Library::scan(&History::default(), dir.to_str().unwrap()));
        s.phase = Phase::Running;
        s.apply(ev(r#"{"ev":"collection","name":"A","kind":"album","tracks":[{"artist":"Enya","title":"Only Time"},{"artist":"Enya","title":"Wild Child"}]}"#));
        assert_eq!(s.tracks[0].state, TState::Have);
        assert!(!s.tracks[0].in_run && s.tracks[1].in_run);
        s.apply(ev(r#"{"ev":"track","i":1,"state":"done","source":"soulseek","peer":"zwem"}"#));
        s.apply(ev(r#"{"ev":"result","files":1,"dir":"/out"}"#));
        // The run is over, but the question is open: no poster yet, a countdown instead.
        assert_eq!(s.phase, Phase::Running);
        assert!(s.countdown.is_some());
        // No focus, no countdown: nothing is skipped behind the user's back.
        s.set_focus(false);
        for k in 1..200 {
            s.tick(k as f64);
        }
        assert_eq!(s.tracks[0].state, TState::Have);
        s.set_focus(true);
        s.decide(0, true);
        assert_eq!(s.tracks[0].state, TState::Pending, "queued for the follow-up run");
        assert!(s.countdown.is_none());
        // The follow-up run: same list, only track 0.
        s.apply(ev(r#"{"ev":"collection","name":"A","kind":"album","tracks":[{"artist":"Enya","title":"Only Time"},{"artist":"Enya","title":"Wild Child"}]}"#));
        assert!(s.tracks[0].in_run && !s.tracks[1].in_run);
        assert_eq!(s.tracks[1].state, TState::Done, "the first run's result stays");
        s.apply(ev(r#"{"ev":"track","i":0,"state":"done","source":"soulseek"}"#));
        s.apply(ev(r#"{"ev":"file","i":0,"path":"/out/Only Time (1).mp3"}"#));
        s.apply(ev(r#"{"ev":"result","files":1,"dir":"/out"}"#));
        assert_eq!(s.phase, Phase::Done);
        assert_eq!(s.files, 2);
        assert_eq!(s.tracks[0].path, "/out/Only Time (1).mp3");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unanswered_questions_are_skipped_after_the_countdown() {
        let ev = |line: &str| backend::parse(line).unwrap();
        let mut s = State::new(None);
        let dir = std::env::temp_dir().join(format!("spotseek-skip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Enya - Only Time.mp3"), b"x").unwrap();
        *s.library.lock().unwrap() = Some(Library::scan(&History::default(), dir.to_str().unwrap()));
        s.phase = Phase::Running;
        s.apply(ev(r#"{"ev":"collection","name":"A","kind":"track","tracks":[{"artist":"Enya","title":"Only Time"}]}"#));
        s.apply(ev(r#"{"ev":"result","dir":"/out"}"#));
        assert_eq!(s.countdown.map(|c| c.1), Some(ASK_ALL_SECS), "everything is already there: the long countdown");
        let mut t = 0.0;
        while s.phase == Phase::Running && t < 60.0 {
            t += 0.1;
            s.tick(t);
        }
        assert_eq!(s.phase, Phase::Done);
        assert!(t >= ASK_ALL_SECS - 0.5, "skipped after {t} s");
        assert_eq!(s.tracks[0].state, TState::Skipped);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stop_keeps_finished_tracks_and_ends_on_the_poster() {
        let ev = |line: &str| backend::parse(line).unwrap();
        let mut s = State::new(None);
        s.phase = Phase::Running;
        s.apply(ev(r#"{"ev":"collection","name":"A","kind":"playlist","tracks":[{"artist":"A","title":"One"},{"artist":"B","title":"Two"}]}"#));
        s.apply(ev(r#"{"ev":"track","i":0,"state":"done","source":"soulseek"}"#));
        // STOP pressed while the backend runs: it converts what is done, then sends a result.
        s.stopping = true;
        s.apply(ev(r#"{"ev":"file","i":0,"path":"/out/One.mp3"}"#));
        s.apply(ev(r#"{"ev":"result","files":1,"dir":"/out","stopped":true}"#));
        assert_eq!(s.phase, Phase::Done, "the poster shows");
        assert!(s.stopped_early && !s.stopping);
        assert_eq!(s.tracks[0].path, "/out/One.mp3");
        assert_eq!(s.tracks[1].state, TState::Missing);
    }

    #[test]
    fn a_quiet_soulseek_shows_the_wait() {
        let ev = |line: &str| backend::parse(line).unwrap();
        let mut s = State::new(None);
        s.phase = Phase::Running;
        s.apply(ev(r#"{"ev":"collection","name":"A","kind":"playlist","tracks":[{"artist":"A","title":"One"},{"artist":"B","title":"Two"}]}"#));
        s.apply(ev(r#"{"ev":"phase","phase":"soulseek"}"#));
        s.tick(5.0);
        assert!(s.soulseek_wait().is_none(), "not after 5 s");
        s.tick(30.0);
        assert!(s.soulseek_wait().is_some_and(|q| q >= 20.0));
        s.apply(ev(r#"{"ev":"track","i":0,"state":"searching"}"#));
        assert!(s.soulseek_wait().is_none(), "progress clears it");
    }

    #[test]
    fn candidates_picks_and_the_preview_file() {
        let ev = |line: &str| backend::parse(line).unwrap();
        let mut s = State::new(None);
        s.phase = Phase::Running;
        s.apply(ev(r#"{"ev":"collection","name":"A","kind":"playlist","manual":true,"tracks":[{"artist":"A","title":"One","duration":200}]}"#));
        assert!(s.manual);
        s.apply(ev(r#"{"ev":"track","i":0,"state":"searching"}"#));
        assert_eq!(s.choosing(), 0, "nothing to pick yet");
        s.apply(ev(r#"{"ev":"candidates","i":0,"settled":true,"cands":[{"id":4,"user":"zwem","file":"One.mp3","ext":"mp3","size":8000000,"bitrate":320,"duration":201,"free":true,"speed":900000,"queue":0,"fit":100,"score":170.0}]}"#));
        assert_eq!(s.choosing(), 1);
        assert_eq!(s.tracks[0].cands[0].user, "zwem");
        // The engine starts the picked file: its growing file is the preview.
        s.apply(ev(r#"{"ev":"track","i":0,"state":"downloading","peer":"zwem","source":"soulseek","total":8000000,"path":"/work/0-4.mp3","format":"mp3","cand":4}"#));
        assert_eq!(s.choosing(), 0);
        assert!(s.tracks[0].previewable());
        let g = s.media.growing("/work/0-4.mp3").unwrap();
        assert_eq!((g.total, g.secs), (8_000_000, 201.0), "the length of the file, not Spotify's");
        s.apply(ev(r#"{"ev":"bytes","i":0,"bytes":4000000,"total":8000000}"#));
        let d = s.detail_of("c0").unwrap();
        assert_eq!(d.path, "/work/0-4.mp3");
        assert!(d.preview);
        assert!((s.media.growing("/work/0-4.mp3").unwrap().playable() - 97.0).abs() < 3.0);
        s.apply(ev(r#"{"ev":"track","i":0,"state":"done","source":"soulseek","peer":"zwem","cand":4}"#));
        assert!(s.media.growing("/work/0-4.mp3").unwrap().complete());
        s.apply(ev(r#"{"ev":"file","i":0,"path":"/out/A - One.mp3","bitrate":320}"#));
        let d = s.detail_of("c0").unwrap();
        assert_eq!(d.path, "/out/A - One.mp3");
        assert!(!d.preview);
    }

    #[test]
    fn a_failed_file_goes_back_to_searching_and_youtube_files_have_no_preview() {
        let ev = |line: &str| backend::parse(line).unwrap();
        let mut s = State::new(None);
        s.phase = Phase::Running;
        s.apply(ev(r#"{"ev":"collection","name":"A","kind":"playlist","tracks":[{"artist":"A","title":"One","duration":200}]}"#));
        s.apply(ev(r#"{"ev":"track","i":0,"state":"downloading","total":8000000,"path":"/work/0-1.mp3","format":"mp3","cand":1}"#));
        s.apply(ev(r#"{"ev":"track","i":0,"state":"searching"}"#));
        assert_eq!(s.tracks[0].cand, None);
        assert!(!s.tracks[0].previewable());
        s.apply(ev(r#"{"ev":"track","i":0,"state":"yt_download","source":"youtube"}"#));
        assert!(s.detail_of("c0").unwrap().path.is_empty());
    }

    /// A real run of the Rust engine (3 tracks): two files stayed queued and the next ones came,
    /// one track had no Soulseek files and came from YouTube.
    #[test]
    fn a_real_rust_engine_run_ends_done() {
        let mut s = State::new(None);
        s.phase = Phase::Running;
        let mut saw_preview = false;
        for line in include_str!("../tests/fixtures/real-run-rust.jsonl").lines() {
            s.apply(backend::parse(line).expect(line));
            saw_preview |= s.tracks.iter().any(|t| t.state == TState::Downloading && t.previewable());
        }
        assert!(saw_preview, "Soulseek downloads give a file to preview");
        assert_eq!(s.phase, Phase::Done);
        assert_eq!(s.count(TState::Done), 3);
        assert!(s.tracks[2].youtube);
        assert!(!s.tracks[0].youtube && s.tracks[0].cand == Some(4), "the third file of track 1 was the one");
        assert!(s.tracks.iter().all(|t| t.path.ends_with(".mp3")));
        assert!(!s.tracks[0].cands.is_empty());
    }

    #[test]
    fn best_for_all_sends_tracks_with_only_failed_files_to_youtube() {
        let ev = |line: &str| backend::parse(line).unwrap();
        let mut s = State::new(None);
        s.phase = Phase::Running;
        s.apply(ev(r#"{"ev":"collection","name":"A","kind":"playlist","manual":true,"tracks":[{"artist":"A","title":"One"},{"artist":"B","title":"Two"}]}"#));
        s.apply(ev(r#"{"ev":"track","i":0,"state":"searching"}"#));
        s.apply(ev(r#"{"ev":"track","i":1,"state":"searching"}"#));
        s.apply(ev(r#"{"ev":"candidates","i":0,"cands":[{"id":1,"user":"a","ext":"mp3","size":9,"fit":100}]}"#));
        s.apply(ev(r#"{"ev":"candidates","i":1,"cands":[{"id":2,"user":"b","ext":"mp3","size":9,"fit":100,"failed":true}]}"#));
        assert_eq!(s.choosing(), 2);
        // No run (a test): pick() needs one to send, so check the choice itself.
        let fallback = s.settings.fallback;
        let chosen: Vec<Option<u32>> = s.tracks.iter().map(|t| t.cands.iter().find(|c| !c.failed).map(|c| c.id)).collect();
        assert_eq!(chosen, vec![Some(1), None]);
        assert!(fallback, "the default sends the second one to YouTube");
    }

    #[test]
    fn notes_are_worded_and_mark_a_dropped_session() {
        let ev = |line: &str| backend::parse(line).unwrap();
        let mut s = State::new(None);
        s.phase = Phase::Running;
        s.apply(ev(r#"{"ev":"collection","name":"A","kind":"playlist","tracks":[{"artist":"Dillom","title":"Coyote"}]}"#));
        s.apply(ev(r#"{"ev":"note","code":"offline","level":"bad","message":"relogged","line":"Soulseek: the account signed in somewhere else"}"#));
        assert!(s.soulseek_down);
        s.apply(ev(r#"{"ev":"note","code":"queued","level":"info","i":0,"peer":"zwem","count":3,"line":"zwem: queued (place 3)"}"#));
        assert_eq!(s.latest_note().unwrap().text, "Dillom – Coyote: zwem put you in a queue (place 3).");
        s.apply(ev(r#"{"ev":"note","code":"reconnected","level":"good","line":"Signed in again"}"#));
        assert!(!s.soulseek_down);
        s.apply(ev(r#"{"ev":"log","line":"technical only"}"#));
        assert_eq!(s.notes.last().unwrap().level, Level::Tech);
        assert_eq!(s.latest_note().unwrap().level, Level::Good, "the footer shows the last note for people");
        assert!(s.notes.iter().any(|n| n.level == Level::Bad && n.text.contains("somewhere else")));
    }

    #[test]
    fn hover_play_starts_pauses_and_remembers_without_sound() {
        let ev = |line: &str| backend::parse(line).unwrap();
        let mut s = State::new(None);
        s.phase = Phase::Running;
        s.apply(ev(r#"{"ev":"collection","name":"A","kind":"playlist","tracks":[{"artist":"A","title":"One","duration":200},{"artist":"B","title":"Two","duration":200}]}"#));
        s.apply(ev(r#"{"ev":"track","i":0,"state":"done","source":"soulseek"}"#));
        s.apply(ev(r#"{"ev":"file","i":0,"path":"/out/A - One.mp3"}"#));
        // Off: hovering does nothing.
        s.hover_track(0, true);
        assert!(s.media.sent().is_empty());
        s.set_hover_play(true);
        assert!(s.notes.last().unwrap().text.contains("mouse"), "the first time says how it works");
        s.hover_track(1, true);
        assert!(s.media.sent().is_empty(), "a track with nothing to play stays silent");
        s.hover_track(0, true);
        assert_eq!(s.media.sent(), ["play /out/A - One.mp3 0.0"]);
        assert_eq!(s.hover_started, Some(0));
        // The silent media does not play, so fake the status, then leave.
        s.media.fake_status(crate::media::Status { path: "/out/A - One.mp3".into(), playing: true, pos: 42.0, dur: 200.0, ..Default::default() });
        s.hover_track(0, false);
        assert_eq!(s.media.sent().last().unwrap(), "pause");
        assert_eq!(s.hover_started, None);
        // Leaving into its own open details does not pause.
        s.open = Some("c0".into());
        s.hover_track(0, true);
        s.hover_track(0, false);
        assert_eq!(s.hover_started, Some(0));
        // Turning it off pauses what the hover started.
        s.set_hover_play(false);
        assert_eq!(s.hover_started, None);
        assert_eq!(s.media.sent().last().unwrap(), "pause");
    }

    #[test]
    fn the_place_a_preview_was_left_goes_to_the_mp3() {
        let ev = |line: &str| backend::parse(line).unwrap();
        let mut s = State::new(None);
        s.phase = Phase::Running;
        s.apply(ev(r#"{"ev":"collection","name":"A","kind":"playlist","tracks":[{"artist":"A","title":"One","duration":200}]}"#));
        s.apply(ev(r#"{"ev":"track","i":0,"state":"downloading","total":8000000,"path":"/work/0-1.mp3","format":"mp3","cand":1}"#));
        s.media.fake_status(crate::media::Status { path: "/work/0-1.mp3".into(), playing: false, pos: 61.0, dur: 200.0, ..Default::default() });
        s.apply(ev(r#"{"ev":"track","i":0,"state":"done","cand":1}"#));
        s.apply(ev(r#"{"ev":"file","i":0,"path":"/out/A - One.mp3"}"#));
        assert_eq!(s.media.position("/out/A - One.mp3"), Some(61.0));
    }
}
