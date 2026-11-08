//! Spotseek: paste a Spotify link, get mp3 files. This is the Rune front end; the work is done
//! by the Rust engine (`spotseek-backend --cli <url> --events`, in `engine/`), which it runs as a
//! child process.
//!
//! `spotseek --snap out.png --scene downloading` renders a screen headless (see `--list-scenes`).

// A window app on Windows: no console window behind it.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod account;
mod backend;
mod gfx;
mod history;
mod i18n;
mod media;
mod model;
mod notes;
mod sdf;
mod settings;
mod view;

use std::time::Duration;

use rune::{App, AppSpec, Key, KeyEvent, Tx};

pub use model::State;
use model::{Phase, TState, Track};

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/Inter-Regular.ttf"),
    include_bytes!("../assets/Inter-Medium.ttf"),
    include_bytes!("../assets/Inter-SemiBold.ttf"),
    include_bytes!("../assets/IBMPlexMono-Regular.ttf"),
    include_bytes!("../assets/IBMPlexMono-Medium.ttf"),
    include_bytes!("../assets/IBMPlexMono-SemiBold.ttf"),
    sdf::ANTON,
];

fn poll(s: &mut State, now: f64, tx: &mut Tx) -> bool {
    let mut changed = s.tick(now);
    let playing = s.media.status().playing;
    if playing {
        // The title moves and the live line follows the sound: about 30 frames a second.
        tx.wake_after(Duration::from_millis(33));
        changed = true;
    } else if s.countdown.is_some() {
        // The countdown moves every frame or two.
        tx.wake_after(Duration::from_millis(100));
        changed = true;
    } else if s.phase == Phase::Running {
        tx.wake_after(Duration::from_millis(if s.demo { 33 } else { 500 }));
    }
    changed
}

fn on_key(s: &mut State, ev: &KeyEvent, _: &mut Tx) -> bool {
    match &ev.key {
        Key::Escape if s.settings_open => s.close_settings(),
        Key::Escape if s.login_open => s.login_open = false,
        Key::Enter if s.settings_open => s.close_settings(),
        Key::Enter if s.login_open => s.sign_in(),
        Key::Escape if s.hover_started.is_some() => s.stop_hover(),
        Key::Escape if s.open.is_some() => s.open = None,
        Key::Text(t) if t == " " && s.open.is_some() && !(s.phase == Phase::Idle && s.link.focused) => {
            if let Some(d) = s.open.clone().and_then(|k| s.detail_of(&k)) {
                if !d.path.is_empty() && !s.demo {
                    s.media.toggle(&d.path);
                }
            }
        }
        Key::Enter if s.phase == Phase::Idle && !s.settings_open && !s.login_open => s.start(),
        Key::Command(',') => s.settings_open = true,
        _ => return false,
    }
    true
}

// ------------------------------------------------------------------ demo scenes

pub const DEMO: [(&str, &str); 40] = [
    ("Daddy Yankee", "Gasolina"), ("Tito El Bambino", "Oye Mi Canto"), ("Don Omar", "Lo Que Paso, Paso"), ("Daddy Yankee", "Mayor Que Yo"),
    ("Wisin & Yandel", "Rakata"), ("Ivy Queen", "Cuentale"), ("Daddy Yankee", "Rompe"), ("Calle 13", "Reggaeton Latino"),
    ("Hector el Father", "Porque Te Vas"), ("Calle 13", "La Botella"), ("Tego Calderon", "Dame Mas"), ("Luny Tunes", "Ven Bailalo"),
    ("Hector y Tito", "Baila Morena"), ("Wisin & Yandel", "Ella Y Yo"), ("Zion", "Siente El Boom"), ("Wisin & Yandel", "Pam Pam"),
    ("Don Omar", "Lo Siento"), ("Tito", "Nuestro Amor"), ("Calle 13", "Tu Y Yo"), ("Wisin", "Corillo"),
    ("Don Omar", "Machuca"), ("Daddy Yankee", "Dame Un Beso"), ("Hector", "La Pista"), ("Tego", "Fue Un Secreto"),
    ("Ivy Queen", "Que Ironia"), ("Zion & Lennox", "Mi Gente"), ("Jacobo", "La Tortura"), ("Rakim & Ken-Y", "Bailando"),
    ("Shakira", "Hips Don't Lie"), ("Arcangel", "Safari"), ("Daddy Yankee", "El Perdedor"), ("Wisin", "Por Ti"),
    ("Hector", "Amor Eterno"), ("Calle 13", "Si Tu Te Vas"), ("Ivy Queen", "Te He Querido"), ("Wisin & Yandel", "El Baile Del Perrito"),
    ("Tego", "La Bomba"), ("Zion", "Suavemente"), ("Mangu", "Merengue"), ("Tito", "Coco Loco"),
];

/// Demo tracks that are already in the folder, so the demo asks about them.
pub const DEMO_HAVE: [usize; 2] = [4, 11];

fn demo(s: &mut State) {
    s.phase = Phase::Running;
    s.stage = "soulseek".into();
    s.name = "Fiesta 2005".into();
    s.kind = "playlist".into();
    s.running_link = "https://open.spotify.com/playlist/37i9dQZF1DX4sWnBi0yS4q".into();
    s.tracks = DEMO.iter().map(|(a, t)| Track { in_run: true, ..Track::new(a, t) }).collect();
}

fn scene_pasted(s: &mut State) {
    s.link.replace_text("https://open.spotify.com/playlist/37i9dQZF1DX4sWnBi0yS4q");
}

fn scene_invalid(s: &mut State) {
    s.link.replace_text("https://music.example.com/what");
}

fn scene_searching(s: &mut State) {
    demo(s);
    for t in s.tracks.iter_mut().take(6) {
        t.state = TState::Searching;
    }
    s.last_log = "[003] SongJob: searching: Daddy Yankee - Gasolina".into();
}

fn scene_downloading(s: &mut State) {
    demo(s);
    for (i, t) in s.tracks.iter_mut().enumerate() {
        t.total = 7_000_000 + (i as u64 * 377_000) % 3_000_000;
        t.state = match i {
            0..=20 if i == 9 || i == 16 => TState::Done,
            0..=20 => TState::Done,
            21 | 22 => TState::Downloading,
            23 => TState::Failed,
            24 => TState::YtDownload,
            25..=28 => TState::Searching,
            _ => TState::Pending,
        };
        if i == 9 || i == 16 || i == 24 {
            t.youtube = true;
        }
        t.bytes = match i {
            21 => t.total * 64 / 100,
            22 => t.total * 23 / 100,
            0..=20 => t.total,
            _ => 0,
        };
        t.peer = if i == 22 { "xXdj_nico".into() } else { "zwem".into() };
    }
    s.speed = 890_000.0;
    s.last_log = "[024] SongJob: downloading: Daddy Yankee - Dame Un Beso: xXdj_nico\\music\\2005\\Dame Un Beso.mp3".into();
}

fn scene_done(s: &mut State) {
    scene_downloading(s);
    for (i, t) in s.tracks.iter_mut().enumerate() {
        t.state = if i == 23 { TState::Missing } else { TState::Done };
        t.bytes = t.total;
        t.in_run = false;
    }
    s.phase = Phase::Done;
    s.stage = "done".into();
    s.files = 39;
}

/// The details of a downloaded track, playing.
fn scene_details(s: &mut State) {
    scene_downloading(s);
    let t = &mut s.tracks[2];
    t.path = "demo://2".into();
    t.bitrate = 320;
    t.duration = 213;
    t.album = "Barrio Fino".into();
    t.started = 1_762_500_000.0;
    t.finished = 1_762_500_000.0 + 23.0;
    s.media.fake_wave("demo://2", 213.0, 7);
    s.media.fake_status(media::Status { path: "demo://2".into(), playing: true, pos: 74.0, dur: 213.0, ..Default::default() });
    s.open = Some("c2".into());
}

/// A link with tracks the user already has: the question, and the countdown.
fn scene_have(s: &mut State) {
    scene_downloading(s);
    for i in [4, 11, 26, 33] {
        let t = &mut s.tracks[i];
        t.state = TState::Have;
        t.have = format!("demo://{i}");
        t.in_run = false;
    }
    s.tracks[30].state = TState::Queued;
    s.tracks[30].in_run = false;
    s.countdown = Some((model::ASK_SECS - 3.6, model::ASK_SECS));
}

/// Every track is already there.
fn scene_have_all(s: &mut State) {
    demo(s);
    for t in &mut s.tracks {
        t.state = TState::Have;
        t.in_run = false;
    }
    s.stage = "done".into();
    s.countdown = Some((model::ASK_ALL_SECS - 2.0, model::ASK_ALL_SECS));
}

/// The poster with the history under it, one history track open.
fn scene_history(s: &mut State) {
    scene_done(s);
    s.tracks.truncate(5);
    s.tracks[3].state = TState::Skipped;
    s.session_id = "now".into();
    let mk = |artist: &str, title: &str, state: &str, src: &str| history::SavedTrack {
        artist: artist.into(),
        title: title.into(),
        state: state.into(),
        source: src.into(),
        peer: if src == "soulseek" { "zwem".into() } else { "YouTube Music".into() },
        bitrate: 320,
        size: 8_400_000,
        duration: 245,
        path: format!("demo://{title}"),
        started: 1_762_400_000.0,
        finished: 1_762_400_031.0,
        ..Default::default()
    };
    s.history.sessions = vec![
        history::Session {
            id: "a".into(),
            name: "A Day Without Rain".into(),
            kind: "album".into(),
            started: 1_762_400_000.0,
            tracks: vec![
                mk("Enya", "Wild Child", "done", "soulseek"),
                mk("Enya", "Only Time", "done", "soulseek"),
                mk("Enya", "Pilgrim", "done", "youtube"),
                mk("Enya", "Silver Inches", "missing", ""),
            ],
            ..Default::default()
        },
        history::Session {
            id: "b".into(),
            name: "Me Das Cada Dia Mas".into(),
            kind: "track".into(),
            started: 1_762_300_000.0,
            tracks: vec![mk("Babasonicos", "Me Das Cada Dia Mas", "done", "soulseek")],
            ..Default::default()
        },
    ];
    s.media.fake_wave("demo://Only Time", 218.0, 3);
    s.open = Some("ha:1".into());
}

/// The history screen from the paste screen, one track open.
fn scene_history_screen(s: &mut State) {
    scene_history(s);
    s.phase = Phase::Idle;
    s.tracks.clear();
    s.show_history = true;
}

/// The Soulseek sign-in window of the first launch.
fn scene_login(s: &mut State) {
    s.account = account::Account::None;
    s.login_open = true;
    s.login_user.replace_text("dj_nico");
    s.login_error = s.lang.t("That name is taken, or the password is wrong.", "Ese nombre ya existe o la contraseña es incorrecta.").into();
}

/// The settings with no account: the dot, and the form.
fn scene_settings_signed_out(s: &mut State) {
    s.account = account::Account::None;
    s.settings_open = true;
}

/// The paste screen with no account: the note and the dot.
fn scene_idle_signed_out(s: &mut State) {
    s.account = account::Account::None;
}

/// A long list during Soulseek's search limit.
fn scene_waiting(s: &mut State) {
    scene_searching(s);
    for t in s.tracks.iter_mut().take(6) {
        t.state = TState::Done;
    }
    // 42 s without progress (the scene clock starts at 0).
    s.last_progress = -42.0;
}

/// Soulseek files for a demo track: a mix of good, queued, slow and wrong ones.
fn demo_cands(t: &Track, seed: usize) -> Vec<backend::Cand> {
    let d = t.duration.max(200);
    let mk = |id: u32, user: &str, ext: &str, size: u64, br: u32, dur: u32, free: bool, speed: u32, queue: u32, fit: u32, sr: u32, bd: u32, name: &str| backend::Cand {
        id,
        user: user.into(),
        file: format!("{name}.{ext}"),
        folder: format!("Music/{}", t.artist),
        ext: ext.into(),
        size,
        bitrate: br,
        duration: dur,
        sample_rate: sr,
        bit_depth: bd,
        free,
        speed,
        queue,
        fit,
        score: 0.0,
        eta: (size / speed.max(40_000) as u64) as u32,
        failed: false,
        known: user == "zwem",
    };
    let name = format!("{:02} - {} - {}", seed % 12 + 1, t.artist, t.title);
    vec![
        mk(0, "zwem", "mp3", 8_200_000, 320, d, true, 1_900_000, 0, 100, 0, 0, &name),
        mk(1, "rodiganhomelab", "flac", 31_400_000, 0, d + 1, true, 2_600_000, 0, 98, 44_100, 16, &name),
        mk(2, "xXdj_nico", "mp3", 8_100_000, 320, d, false, 3_100_000, 14, 100, 0, 0, &name),
        mk(3, "hfegetude", "mp3", 4_900_000, 192, d, true, 410_000, 0, 95, 0, 0, &name),
        mk(4, "djcotter", "mp3", 8_900_000, 320, d + 74, true, 1_200_000, 0, 62, 0, 0, &format!("{} - {} (Extended Mix)", t.artist, t.title)),
    ]
}

/// Manual mode: the songs wait for a pick; one is open with its files.
fn scene_manual(s: &mut State) {
    demo(s);
    s.manual = true;
    s.settings.manual = true;
    for (i, t) in s.tracks.iter_mut().enumerate() {
        t.duration = 200 + (i as u32 * 7) % 60;
        match i {
            0 | 1 => {
                t.cands = demo_cands(t, i);
                t.settled = true;
                t.cand = Some(0);
                t.state = TState::Done;
                t.total = t.cands[0].size;
                t.bytes = t.total;
            }
            2..=9 => {
                t.cands = demo_cands(t, i);
                t.settled = i != 3;
                t.state = TState::Searching;
            }
            10..=13 => t.state = TState::Searching,
            _ => {}
        }
    }
    s.open = Some("c3".into());
    s.media.fake_wave("demo://none", 1.0, 1);
}

/// A song that downloads, open: its preview plays the part on disk; the rest is a faint line.
fn scene_preview(s: &mut State) {
    scene_downloading(s);
    let t = &mut s.tracks[21];
    t.duration = 236;
    t.cands = demo_cands(t, 21);
    t.settled = true;
    t.cand = Some(0);
    t.total = t.cands[0].size;
    t.bytes = t.total * 58 / 100;
    t.part = "demo-part://21".into();
    t.format = "mp3".into();
    t.bitrate = 320;
    t.peer = "zwem".into();
    t.started = history::now() - 9.0;
    let (part, bytes, total) = (t.part.clone(), t.bytes, t.total);
    s.media.set_growing(&part, bytes, total, 236.0);
    s.media.fake_wave(&part, 236.0, 5);
    s.media.fake_status(media::Status { path: part, playing: true, pos: 61.0, dur: 236.0, ..Default::default() });
    s.open = Some("c21".into());
}

/// A song plays: its row has the speaker, the moving title and the live line; the footer has
/// the small player.
fn scene_playing(s: &mut State) {
    scene_downloading(s);
    let t = &mut s.tracks[2];
    t.path = "demo://2".into();
    t.title = "Lo Que Paso, Paso (Remasterizado 2025)".into();
    s.media.fake_levels(3);
    s.media.fake_status(media::Status { path: "demo://2".into(), playing: true, pos: 74.0, dur: 213.0, ..Default::default() });
    s.now = 2.3;
}

/// The log, open: what happened in plain words, the marked ones explain the waits.
fn scene_log(s: &mut State) {
    scene_downloading(s);
    s.session_started = history::now() - 400.0;
    let lines = [
        r#"{"ev":"note","code":"signed_in","level":"good","line":"Signed in to Soulseek in 1.2 s"}"#,
        r#"{"ev":"note","code":"found","level":"info","i":0,"count":36,"secs":2,"line":"x"}"#,
        r#"{"ev":"note","code":"queued","level":"info","i":3,"peer":"rawzik","count":1,"line":"rawzik: queued (place 1)"}"#,
        r#"{"ev":"note","code":"peer_failed","level":"warn","i":3,"peer":"rawzik","message":"queued","line":"x"}"#,
        r#"{"ev":"note","code":"downloaded","level":"good","i":3,"peer":"zwem","secs":11,"line":"x"}"#,
        r#"{"ev":"note","code":"search_limit","level":"warn","count":42,"secs":95,"line":"x"}"#,
        r#"{"ev":"note","code":"no_files","level":"warn","i":9,"line":"x"}"#,
        r#"{"ev":"note","code":"offline","level":"bad","message":"relogged","line":"x"}"#,
        r#"{"ev":"note","code":"reconnecting","level":"warn","secs":60,"line":"x"}"#,
    ];
    for l in lines {
        s.apply(backend::parse(l).unwrap());
    }
    s.log_open = true;
}

fn scene_failed(s: &mut State) {
    demo(s);
    s.phase = Phase::Failed;
    s.error = "spotify: this playlist is private or does not exist".into();
}

fn scene_settings(s: &mut State) {
    s.settings_open = true;
}

fn main() {
    rune::prewarm_gpu();
    let spec = AppSpec {
        min_size: Some((820.0, 620.0)),
        scenes: &[
            ("pasted", scene_pasted),
            ("invalid", scene_invalid),
            ("searching", scene_searching),
            ("downloading", scene_downloading),
            ("done", scene_done),
            ("failed", scene_failed),
            ("details", scene_details),
            ("have", scene_have),
            ("have-all", scene_have_all),
            ("history", scene_history),
            ("history-screen", scene_history_screen),
            ("login", scene_login),
            ("settings-signed-out", scene_settings_signed_out),
            ("idle-signed-out", scene_idle_signed_out),
            ("waiting", scene_waiting),
            ("settings", scene_settings),
            ("manual", scene_manual),
            ("preview", scene_preview),
            ("playing", scene_playing),
            ("log", scene_log),
        ],
        fonts: FONTS,
        families: ("Inter", "IBM Plex Mono"),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Spotseek", (1100.0, 780.0), || State::new(None), view::view)
    };
    let app = App::new(spec);
    let handle = app.handle();
    app.init(move || {
        let mut s = State::new(Some(handle));
        s.ask_login_at_launch();
        s
    })
        .on_close(|s: &mut State, _| {
            if let Some(mut r) = s.run.take() {
                r.stop();
            }
            s.media.stop();
        })
        .on_focus(|s: &mut State, focused, _| s.set_focus(focused))
        .compositor(gfx::Gfx::default())
        .run();
}
