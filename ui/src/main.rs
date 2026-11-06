//! Spotseek: paste a Spotify link, get mp3 files. This is the Rune front end; the work is done
//! by the Go backend (`spotseek-backend --cli <url> --events`), which it runs as a child process.
//!
//! `spotseek --snap out.png --scene downloading` renders a screen headless (see `--list-scenes`).

// A window app on Windows: no console window behind it.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod backend;
mod gfx;
mod model;
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
    let changed = s.tick(now);
    if s.phase == Phase::Running {
        tx.wake_after(Duration::from_millis(if s.demo { 33 } else { 500 }));
    }
    changed
}

fn on_key(s: &mut State, ev: &KeyEvent, _: &mut Tx) -> bool {
    match &ev.key {
        Key::Escape if s.settings_open => s.close_settings(),
        Key::Enter if s.phase == Phase::Idle && !s.settings_open => s.start(),
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

fn demo(s: &mut State) {
    s.phase = Phase::Running;
    s.stage = "soulseek".into();
    s.name = "Fiesta 2005".into();
    s.kind = "playlist".into();
    s.running_link = "https://open.spotify.com/playlist/37i9dQZF1DX4sWnBi0yS4q".into();
    s.tracks = DEMO.iter().map(|(a, t)| Track::new(a, t)).collect();
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
    }
    s.phase = Phase::Done;
    s.stage = "done".into();
    s.files = 39;
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
            ("settings", scene_settings),
        ],
        fonts: FONTS,
        families: ("Inter", "IBM Plex Mono"),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Spotseek", (1100.0, 780.0), || State::new(None), view::view)
    };
    let app = App::new(spec);
    let handle = app.handle();
    app.init(move || State::new(Some(handle)))
        .on_close(|s: &mut State, _| {
            if let Some(mut r) = s.run.take() {
                r.stop();
            }
        })
        .compositor(gfx::Gfx::default())
        .run();
}
