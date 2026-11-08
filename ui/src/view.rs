//! The screens: paste a link, watch it work, look at the poster and the history. Every track
//! opens a details panel with its cover, where it came from, and a player.

use rune::color::hex;
use rune::{
    Color, Curve, Cursor, Cx, Drag, El, Easing, Family, FileDialogOptions, Phase as DragPhase, ScrollSpec, Spring, SurfaceFit, TextStyle, Transition, Tx, col, el,
    row, text, text_input,
};
use rune_motion::Cycle;

use crate::account::Account;
use crate::gfx;
use crate::i18n::{self, Lang};
use crate::media::{Cover, Wave, clock};
use crate::model::{Detail, Phase, State, TState, Track};

// ------------------------------------------------------------------ look

fn c(s: &str) -> Color {
    hex(s).unwrap_or(Color::BLACK)
}
pub fn paper() -> Color {
    c("#f3f2ee")
}
fn ink() -> Color {
    c("#0b0b0b")
}
fn muted() -> Color {
    c("#8d8d8a")
}
fn red() -> Color {
    c("#e8321e")
}
fn yellow() -> Color {
    c("#f7e81a")
}
/// The poster colours, in order.
fn stripe(i: usize) -> Color {
    const S: [&str; 7] = ["#e8321e", "#1d3fd6", "#f7e81a", "#ff3e9c", "#5fdcb4", "#ff6b12", "#2e9e4f"];
    c(S[i % S.len()])
}

fn mono(size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight).mono()
}
fn display(size: f32) -> TextStyle {
    TextStyle::new(size, 400.0).family(Family::Named("Anton")).lh(size * 1.0)
}

const BLINK: Cycle = Cycle { period: 0.9, phase: 0.0, stops: &[(0.0, 1.0), (0.55, 1.0), (0.56, 0.15), (1.0, 0.15)], still: 1.0, fps: 6.0 };
/// The ring of the "something to do" dot: it grows faint, again and again.
const PING: Cycle = Cycle { period: 1.4, phase: 0.0, stops: &[(0.0, 0.9), (0.7, 0.0), (1.0, 0.0)], still: 0.6, fps: 30.0 };

/// A red dot with a pulsing ring: there is something to do here (sign in to Soulseek).
fn ping(key: &str) -> El {
    el().key(key).size(10.0, 10.0).inert()
        .child(el().absolute(-5.0, -5.0).size(20.0, 20.0).radius(10.0).border(2.0, red()).cycle(PING))
        .child(el().size(10.0, 10.0).radius(5.0).fill(red()))
}

fn button(key: &str, label: &str, dark: bool) -> El {
    let (bg, fg) = if dark { (ink(), paper()) } else { (Color::TRANSPARENT, ink()) };
    row().key(key).h(46.0).px(20.0).center().fill(bg).border(2.0, ink()).cursor(Cursor::Pointer).role("button").label(label)
        .hover(red())
        .child(text(label).font(mono(14.0, 600.0)).color(fg))
}

fn link_text(key: &str, label: &str) -> El {
    text(label).key(key).font(mono(14.0, 600.0)).color(ink()).cursor(Cursor::Pointer).role("button").label(label)
}

// ------------------------------------------------------------------ root

pub fn view(s: &State, cx: &mut Cx<State>) -> El {
    let size = cx.layout_size("root").unwrap_or([1100.0, 760.0]);
    let body = match s.phase {
        Phase::Idle if s.show_history => history_screen(s, size),
        Phase::Idle => idle(s, size),
        Phase::Running | Phase::Failed => working(s, size),
        Phase::Done => poster(s, size),
    };
    // Room for the window buttons, which sit over the content (they end about 26 pt down).
    let inset = cx.titlebar_inset().min(26.0);
    let mut root = col().key("root").w_full().h_full().pt(inset).fill(paper())
        .on_paste(|s: &mut State, pasted: &str, _: &mut Tx| s.paste(pasted))
        .child(grid(size))
        .child(body);
    if s.settings_open {
        root = root.child(settings_panel(s));
    } else if s.login_open {
        root = root.child(login_window(s));
    }
    root
}

/// Faint graph paper behind everything.
fn grid(size: [f32; 2]) -> El {
    let step = 44.0;
    let line = ink().alpha(0.055);
    let mut g = el().absolute(0.0, 0.0).size(size[0], size[1]).inert();
    let mut x = step;
    while x < size[0] {
        g = g.child(el().absolute(x, 0.0).size(1.0, size[1]).fill(line));
        x += step;
    }
    let mut y = step;
    while y < size[1] {
        g = g.child(el().absolute(0.0, y).size(size[0], 1.0).fill(line));
        y += step;
    }
    g
}

fn headline(s: &State, label: &str) -> El {
    el().key("headline").matched("headline").motion(Spring::new(0.75, 0.12)).effect(gfx::HEADLINE).effect_data(s.headline()).w_full().role("heading").label(label)
}

// ------------------------------------------------------------------ 1. paste

fn idle(s: &State, size: [f32; 2]) -> El {
    let l = s.lang;
    let ok = s.link_url().is_some();
    let has_text = !s.link.text.trim().is_empty();
    let field_w = (size[0] - 220.0).clamp(280.0, 680.0);
    let field = text_input(&s.link, mono(24.0, 500.0)).key("link").single_line().w_full().caret(ink(), red().alpha(0.25))
        .color(ink()).placeholder("https://open.spotify.com/…", muted())
        .on_edit(|s: &mut State| &mut s.link)
        .on_submit(|s: &mut State, _: &mut Tx| s.start());
    let underline = el().key("underline").w_full().h(4.0)
        .fill(if ok { red() } else if has_text { ink() } else { ink().alpha(0.25) })
        .motion(Spring::new(0.3, 0.0));
    let go = row().key("go").size(58.0, 58.0).center().cursor(Cursor::Pointer).role("button").label("Download")
        .fill(if ok { ink() } else { ink().alpha(0.12) })
        .hover(red())
        .on_press(|s: &mut State| s.start())
        .child(text("→").font(mono(30.0, 600.0)).color(paper()));
    let hint = if !s.notice.is_empty() {
        s.notice.clone()
    } else if has_text && !ok {
        l.t("THAT IS NOT A SPOTIFY LINK. PLAYLISTS, ALBUMS AND TRACKS WORK.", "ESO NO ES UN LINK DE SPOTIFY. FUNCIONAN PLAYLISTS, ÁLBUMES Y CANCIONES.").into()
    } else if ok {
        l.t("PRESS ENTER ↵ TO DOWNLOAD", "APRETÁ ENTER ↵ PARA DESCARGAR").into()
    } else {
        format!("{} {} {}", l.t("PASTE WITH", "PEGÁ CON"), l.paste_keys(), l.t("ANYWHERE · PLAYLIST, ALBUM OR TRACK", "EN CUALQUIER LADO · PLAYLIST, ÁLBUM O CANCIÓN"))
    };
    let no_account = s.account == Account::None;
    let hint_color = if has_text && !ok { red() } else { muted() };
    let footer = row().w_full().h(40.0).center_y().justify_between()
        .child(text(format!("{} · MP3 {}", s.settings.short_dir(), s.settings.bitrate)).key("where").font(mono(14.0, 500.0)).color(ink())
            .cursor(Cursor::Pointer).on_press(|s: &mut State| s.settings_open = true))
        .child(row().gap(28.0).center_y()
            .child(pick_toggle(s))
            .child(link_text("open-history", l.t("HISTORY", "HISTORIAL")).on_press(|s: &mut State| {
                s.show_history = true;
                s.open = None;
            }))
            .child(row().key("open-settings-row").gap(10.0).center_y()
                .child(if no_account { ping("settings-ping") } else { el().key("settings-ping") })
                .child(link_text("open-settings", l.t("SETTINGS", "AJUSTES")).on_press(|s: &mut State| s.settings_open = true))));
    // Without an account everything comes from YouTube Music: say so, and offer to sign in.
    let account_note = if no_account {
        row().key("no-account").gap(10.0).center_y().cursor(Cursor::Pointer).role("button")
            .on_press(|s: &mut State| s.login_open = true)
            .child(ping("no-account-ping"))
            .child(text(l.t("NOT SIGNED IN TO SOULSEEK · DOWNLOADS COME FROM YOUTUBE MUSIC (SLOWER) · SIGN IN →", "NO INICIASTE SESIÓN EN SOULSEEK · LAS DESCARGAS VIENEN DE YOUTUBE MUSIC (MÁS LENTO) · ENTRÁ →"))
                .font(mono(12.0, 600.0)).color(red()))
    } else {
        el().key("no-account")
    };
    col().key("idle").w_full().h_full().px(44.0).pt(36.0).pb(14.0).gap(18.0)
        .child(headline(s, l.t("Paste a Spotify link", "Pegá un link de Spotify")).grow(1.0).min_h(160.0))
        .child(row().w_full().center().gap(18.0).pt(10.0)
            .child(col().w(field_w).gap(6.0).child(field).child(underline))
            .child(go))
        .child(row().w_full().center().child(text(hint).key("hint").font(mono(12.0, 500.0)).color(hint_color)))
        .child(row().w_full().center().child(account_note))
        .child(el().h(8.0))
        .child(footer)
}

/// AUTO / MANUAL: who picks the Soulseek file for each song.
fn pick_toggle(s: &State) -> El {
    let l = s.lang;
    let part = |key: &str, label: &str, on: bool, manual: bool| {
        row().key(key).h(24.0).px(8.0).center().cursor(Cursor::Pointer).role("button").label(label)
            .fill(if on { ink() } else { Color::TRANSPARENT })
            .on_press(move |s: &mut State| {
                s.settings.manual = manual;
                s.save_settings();
            })
            .child(text(label.to_string()).font(mono(12.0, 600.0)).color(if on { paper() } else { ink() }))
    };
    row().key("pick-mode").gap(8.0).center_y()
        .child(text(l.t("FILES:", "ARCHIVOS:")).font(mono(12.0, 600.0)).color(muted()))
        .child(row().border(2.0, ink())
            .child(part("pick-auto", l.t("AUTO", "AUTO"), !s.settings.manual, false))
            .child(part("pick-manual", l.t("I PICK", "ELIJO YO"), s.settings.manual, true)))
}

// ------------------------------------------------------------------ 2. working

fn fmt_speed(b: f32) -> String {
    if b <= 0.0 {
        "—".into()
    } else if b >= 1_000_000.0 {
        format!("{:.1} MB/s", b / 1_000_000.0)
    } else {
        format!("{:.0} KB/s", b / 1000.0)
    }
}

fn fmt_size(b: u64) -> String {
    if b >= 1_000_000 { format!("{:.1} MB", b as f64 / 1_000_000.0) } else { format!("{:.0} KB", b as f64 / 1000.0) }
}

fn status(s: &State) -> &'static str {
    let l = s.lang;
    if s.phase == Phase::Failed {
        return l.t("FAILED", "FALLÓ");
    }
    if s.stopping {
        return l.t("STOPPING · SAVING THE FINISHED SONGS", "FRENANDO · GUARDANDO LAS CANCIONES TERMINADAS");
    }
    if s.soulseek_wait().is_some() {
        return l.t("WAITING FOR SOULSEEK", "ESPERANDO A SOULSEEK");
    }
    if s.choosing() > 0 && !s.downloading() {
        return l.t("PICK A FILE FOR EACH SONG", "ELEGÍ UN ARCHIVO PARA CADA CANCIÓN");
    }
    let running = s.tracks.iter().any(|t| t.in_run);
    if !s.tracks.is_empty() && !running && s.asking() > 0 {
        return l.t("ALREADY IN YOUR FOLDER", "YA ESTÁ EN TU CARPETA");
    }
    match s.stage.as_str() {
        "spotify" => l.t("READING SPOTIFY", "LEYENDO SPOTIFY"),
        "soulseek" if s.downloading() => l.t("DOWNLOADING FROM SOULSEEK", "DESCARGANDO DE SOULSEEK"),
        "soulseek" => l.t("SEARCHING SOULSEEK", "BUSCANDO EN SOULSEEK"),
        "youtube" if s.downloading() => l.t("DOWNLOADING FROM YOUTUBE MUSIC", "DESCARGANDO DE YOUTUBE MUSIC"),
        "youtube" => l.t("SEARCHING YOUTUBE MUSIC", "BUSCANDO EN YOUTUBE MUSIC"),
        "convert" => l.t("CONVERTING TO MP3", "CONVIRTIENDO A MP3"),
        "done" => l.t("FINISHING", "TERMINANDO"),
        _ => l.t("CONNECTING", "CONECTANDO"),
    }
}

fn stat(key: &str, label: &str, value: String) -> El {
    col().key(key).px(18.0).gap(3.0).items_end()
        .child(text(label).font(mono(11.0, 600.0)).color(muted()))
        .child(text(value).font(mono(15.0, 600.0)).color(ink()))
}

/// "FROM ZWEM ⟶", "FROM ZWEM +2 PEERS ⟶".
fn peer_label(s: &State) -> String {
    let l = s.lang;
    let peers = s.peers();
    match peers.len() {
        0 if s.stage == "convert" => format!("{} {}/{}", l.t("CONVERTING", "CONVIRTIENDO"), s.convert.0, s.convert.1),
        0 => l.t("WAITING FOR PEERS", "ESPERANDO USUARIOS").into(),
        1 => format!("{} {} ⟶", l.t("FROM", "DE"), peers[0].to_uppercase()),
        n => format!("{} {} +{} {} ⟶", l.t("FROM", "DE"), peers[0].to_uppercase(), n - 1, l.t("PEERS", "USUARIOS")),
    }
}

fn working(s: &State, size: [f32; 2]) -> El {
    let l = s.lang;
    let n = s.tracks.len();
    let coll = if n == 0 { "…".to_string() } else { format!("{}: {} · {}", l.kind(&s.kind), s.name.to_uppercase(), l.tracks(n)) };
    let header = row().key("header").w_full().h(68.0).px(24.0).center_y().justify_between()
        .child(col().gap(4.0).shrink().min_w(0.0)
            .child(row().gap(8.0).center_y()
                .child(el().size(9.0, 9.0).radius(5.0).fill(if s.phase == Phase::Failed { red() } else { ink() }).cycle(BLINK))
                .child(text(status(s)).key("status").font(mono(15.0, 600.0)).color(ink())))
            .child(text(s.running_link.clone()).font(mono(12.0, 400.0)).color(muted()).truncate()))
        .child(row().center_y()
            .child(stat("stat-coll", l.t("COLLECTION", "COLECCIÓN"), coll))
            .child(el().w(2.0).h(34.0).fill(ink()))
            .child(stat("stat-progress", l.t("PROGRESS", "PROGRESO"), format!("{}/{}", s.finished(), n)))
            .child(el().w(2.0).h(34.0).fill(ink()))
            .child(stat("stat-speed", l.t("SPEED", "VELOCIDAD"), fmt_speed(s.speed))));
    let rule = || el().w_full().h(2.0).fill(ink());

    let pct = (s.progress() * 100.0).round() as u32;
    let left = s.tracks.iter().filter(|t| t.in_run && !matches!(t.state, TState::Done | TState::Missing)).count();
    let bar = col().key("bar-block").w_full().gap(6.0)
        .child(row().w_full().justify_between()
            .child(text(format!("⟵ {}", s.settings.short_dir().to_uppercase())).font(mono(11.0, 600.0)).color(muted()))
            .child(text(peer_label(s)).key("peer").font(mono(11.0, 600.0)).color(ink())))
        .child(el().key("bar").w_full().h(62.0).radius(31.0).border(2.5, ink()).fill(Color::WHITE)
            .child(el().key("bar-fx").effect(gfx::BAR).effect_data(s.bar()).absolute(0.0, 0.0).w_full().h_full().label(format!("{pct} percent"))))
        .child(row().w_full().justify_between()
            .child(text(format!("{pct}%")).key("pct").font(display(30.0)).color(ink()))
            .child(text(format!("{} {} · {} {}", s.count(TState::Done), l.t("DONE", "LISTAS"), left, l.t("LEFT", "FALTAN"))).font(mono(12.0, 600.0)).color(ink())));

    let mut below = col().key("below").w_full().grow(1.0).min_h(0.0).gap(10.0);
    if s.phase == Phase::Failed {
        let mut failed = col().key("failed").w_full().gap(16.0).pt(10.0);
        if let Some(h) = i18n::hint(l, &s.error) {
            failed = failed.child(text(h).key("error-hint").font(mono(15.0, 600.0)).color(ink()).wrap());
        }
        below = below.child(failed
            .child(text(if s.error.is_empty() { l.t("Something went wrong.", "Algo salió mal.").into() } else { s.error.clone() }).font(mono(13.0, 500.0)).color(red()).wrap())
            .child(row().gap(14.0)
                .child(button("retry", l.t("TRY AGAIN", "PROBÁ DE NUEVO"), true).on_press(|s: &mut State| s.start()))
                .child(button("new-link", l.t("NEW DOWNLOAD", "NUEVA DESCARGA"), false).on_press(|s: &mut State| s.new_link()))));
    } else {
        if s.login_problem || s.account == Account::None {
            below = below.child(account_banner(s));
        }
        if let Some(quiet) = s.soulseek_wait() {
            below = below.child(wait_banner(s, quiet));
        }
        if s.asking() > 0 {
            below = below.child(ask_banner(s));
        }
        if s.choosing() > 0 {
            below = below.child(choose_banner(s));
        }
        below = below.child(track_grid(s, size[0] - 56.0));
    }

    let footer = row().key("footer").w_full().h(46.0).px(24.0).gap(18.0).center_y()
        .child(row().key("stop").gap(8.0).center_y().cursor(Cursor::Pointer).role("button").label("Stop")
            .on_press(|s: &mut State| if s.phase == Phase::Running { s.stop() } else { s.new_link() })
            .child(el().size(16.0, 16.0).radius(8.0).border(2.0, ink()))
            .child(text(match (s.phase, s.stopping) {
                (Phase::Running, true) => l.t("STOP NOW", "FRENAR YA"),
                (Phase::Running, false) => l.t("STOP", "FRENAR"),
                _ => l.t("BACK", "VOLVER"),
            }).font(mono(13.0, 600.0)).color(ink())))
        .child(text(s.last_log.clone()).key("log").font(mono(11.0, 400.0)).color(muted()).truncate().grow(1.0).min_w(0.0))
        .child(text("SPOTSEEK").font(mono(13.0, 600.0)).color(ink()));

    let hl = (size[1] * 0.25).clamp(110.0, 240.0);
    col().key("working").w_full().h_full()
        .child(header)
        .child(rule())
        .child(col().w_full().grow(1.0).min_h(0.0).px(28.0).pt(18.0).gap(16.0)
            .child(headline(s, status(s)).h(hl))
            .child(bar)
            .child(below))
        .child(rule())
        .child(footer)
}

/// "You already have these": the answer for all of them, and the countdown to "skip".
fn ask_banner(s: &State) -> El {
    let l = s.lang;
    let n = s.asking();
    let dir = s.settings.short_dir().to_uppercase();
    let mut msg = match (l, n) {
        (Lang::En, 1) => format!("1 TRACK IS ALREADY IN {dir}"),
        (Lang::En, n) => format!("{n} TRACKS ARE ALREADY IN {dir}"),
        (Lang::Es, 1) => format!("1 CANCIÓN YA ESTÁ EN {dir}"),
        (Lang::Es, n) => format!("{n} CANCIONES YA ESTÁN EN {dir}"),
    };
    let left = s.countdown_left();
    if let Some(secs) = left {
        msg += &format!(" · {} {}", l.t("SKIPPING IN", "SE SALTEAN EN"), secs.ceil() as u32);
    } else {
        msg += l.t(" · THEY ARE SKIPPED WHEN THE REST IS DONE", " · SE SALTEAN CUANDO TERMINE EL RESTO");
    }
    let frac = s.countdown.map_or(1.0, |(l, len)| (l.max(0.0) / len) as f32);
    let small = |key: &str, label: &str, dark: bool| {
        let (bg, fg) = if dark { (ink(), yellow()) } else { (Color::TRANSPARENT, ink()) };
        row().key(key).h(30.0).px(12.0).center().fill(bg).border(2.0, ink()).cursor(Cursor::Pointer).role("button").label(label)
            .hover(red())
            .child(text(label).font(mono(12.0, 600.0)).color(fg))
    };
    col().key("ask").w_full().fill(yellow()).border(2.0, ink())
        .transition(Transition::offset(0.0, -10.0).combined(Transition::opacity()))
        .child(row().w_full().h(46.0).px(14.0).gap(12.0).center_y()
            .child(text(msg).key("ask-msg").font(mono(13.0, 600.0)).color(ink()).truncate().grow(1.0).min_w(0.0))
            .child(small("ask-again", l.t("DOWNLOAD AGAIN", "DESCARGAR DE NUEVO"), false).on_press(|s: &mut State| s.decide_all(true)))
            .child(small("ask-skip", l.t("SKIP", "SALTEAR"), true).on_press(|s: &mut State| s.decide_all(false))))
        // The time left, as a bar that runs out.
        .child(el().key("ask-time").h(4.0).w_frac(frac).fill(ink()).motion(Spring::new(0.15, 0.0)))
}

/// Manual mode: songs wait for a pick. Open one to see its files, or take the best for all.
fn choose_banner(s: &State) -> El {
    let l = s.lang;
    let n = s.choosing();
    let msg = match (l, n) {
        (Lang::En, 1) => "1 SONG WAITS FOR YOUR PICK · CLICK IT TO SEE ITS FILES".to_string(),
        (Lang::En, n) => format!("{n} SONGS WAIT FOR YOUR PICK · CLICK ONE TO SEE ITS FILES"),
        (Lang::Es, 1) => "1 CANCIÓN ESPERA QUE ELIJAS · TOCALA PARA VER SUS ARCHIVOS".to_string(),
        (Lang::Es, n) => format!("{n} CANCIONES ESPERAN QUE ELIJAS · TOCÁ UNA PARA VER SUS ARCHIVOS"),
    };
    row().key("choose").w_full().h(40.0).px(14.0).gap(12.0).center_y().fill(yellow()).border(2.0, ink())
        .transition(Transition::offset(0.0, -10.0).combined(Transition::opacity()))
        .child(el().size(9.0, 9.0).radius(5.0).fill(ink()).cycle(BLINK))
        .child(text(msg).key("choose-msg").font(mono(12.0, 600.0)).color(ink()).truncate().grow(1.0).min_w(0.0))
        .child(row().key("choose-best").h(28.0).px(10.0).center().fill(ink()).cursor(Cursor::Pointer).role("button").hover(red())
            .on_press(|s: &mut State| s.pick_best_for_all())
            .child(text(l.t("BEST FILE FOR ALL", "EL MEJOR PARA TODAS")).font(mono(12.0, 600.0)).color(yellow())))
}

/// No Soulseek account, or the engine could not sign in: everything comes from YouTube Music.
fn account_banner(s: &State) -> El {
    let l = s.lang;
    let msg = if s.login_problem {
        l.t("SOULSEEK COULD NOT SIGN IN · USING YOUTUBE MUSIC · CHECK YOUR ACCOUNT", "SOULSEEK NO PUDO INICIAR SESIÓN · SE USA YOUTUBE MUSIC · REVISÁ TU CUENTA")
    } else {
        l.t("NO SOULSEEK ACCOUNT · EVERYTHING COMES FROM YOUTUBE MUSIC (SLOWER)", "SIN CUENTA DE SOULSEEK · TODO VIENE DE YOUTUBE MUSIC (MÁS LENTO)")
    };
    row().key("account-banner").w_full().h(40.0).px(14.0).gap(12.0).center_y().border(2.0, red())
        .child(ping("account-banner-ping"))
        .child(text(msg).font(mono(12.0, 600.0)).color(red()).truncate().grow(1.0).min_w(0.0))
        .child(row().key("account-banner-go").h(28.0).px(10.0).center().fill(red()).cursor(Cursor::Pointer).role("button")
            .on_press(|s: &mut State| s.settings_open = true)
            .child(text(l.t("ACCOUNT", "CUENTA")).font(mono(12.0, 600.0)).color(paper())))
}

/// Soulseek is quiet because of its search limit: say so, so it does not look stuck.
fn wait_banner(s: &State, quiet: f64) -> El {
    let l = s.lang;
    let msg = format!(
        "{} · {}",
        l.t(
            "WAITING FOR SOULSEEK: IT ALLOWS 34 SEARCHES EVERY 3:40, SO LONG LISTS PAUSE. IT GOES ON BY ITSELF",
            "ESPERANDO A SOULSEEK: PERMITE 34 BÚSQUEDAS CADA 3:40, ASÍ QUE LAS LISTAS LARGAS HACEN PAUSAS. SIGUE SOLO",
        ),
        clock(quiet)
    );
    row().key("wait-banner").w_full().h(40.0).px(14.0).gap(12.0).center_y().border(2.0, ink()).fill(paper())
        .transition(Transition::offset(0.0, -10.0).combined(Transition::opacity()))
        .child(el().size(9.0, 9.0).radius(5.0).fill(ink()).cycle(BLINK))
        .child(text(msg).key("wait-msg").font(mono(12.0, 600.0)).color(ink()).truncate().grow(1.0).min_w(0.0))
}

/// Tracks in rows, left to right. Under each row is a band that opens to show the details of
/// a track in that row.
fn track_grid(s: &State, width: f32) -> El {
    let n = s.tracks.len();
    if n == 0 {
        return col().key("tracks").w_full().grow(1.0).center()
            .child(text(s.lang.t("READING THE TRACK LIST…", "LEYENDO LA LISTA…")).font(mono(13.0, 600.0)).color(muted()).cycle(BLINK));
    }
    let cols = ((width / 250.0).floor() as usize).clamp(1, 6);
    let cw = width / cols as f32;
    let mut grid = col().key("track-rows").w_full().gap(2.0);
    for r in 0..n.div_ceil(cols) {
        let mut line = row().key(format!("row-{r}")).w_full();
        let mut open = None;
        for i in r * cols..((r + 1) * cols).min(n) {
            let key = format!("c{i}");
            if s.open.as_deref() == Some(key.as_str()) {
                open = Some(key);
            }
            line = line.child(el().key(format!("slot-{i}")).w(cw).pr(10.0).child(cell(s.lang, i, &s.tracks[i], s.open.as_deref() == Some(format!("c{i}").as_str()), s.manual)));
        }
        grid = grid.child(line).child(band(s, format!("band-{r}"), open, width - 10.0));
    }
    col().key("tracks").w_full().grow(1.0).min_h(0.0).pb(10.0).scroll(ScrollSpec { stick_bottom: false }).child(grid)
}

/// Accordion motion: snappy. Almost all of the height arrives in the first frames, then a
/// short soft landing. The content is there from the first frame (no fade).
const EASE_OUT: Easing = Easing::cubic(0.16, 1.0, 0.3, 1.0);
const OPEN_SECS: f32 = 0.16;

fn open_on_press(key: String) -> impl Fn(&mut State, &mut Tx) + 'static {
    move |s: &mut State, tx: &mut Tx| {
        tx.motion_curve(Curve::tween(OPEN_SECS, EASE_OUT));
        s.toggle_open(key.clone());
    }
}

fn cell(l: Lang, i: usize, t: &Track, open: bool, manual: bool) -> El {
    let title = t.title.to_uppercase();
    let choosing = t.choosing(manual);
    let (bg, fg, chip): (Color, Color, String) = match t.state {
        _ if choosing => (yellow(), ink(), l.t("PICK ↓", "ELEGÍ ↓").into()),
        TState::Pending => (Color::TRANSPARENT, muted(), String::new()),
        TState::Searching => (Color::TRANSPARENT, ink(), "…".into()),
        TState::Downloading => {
            let p = if t.total > 0 { t.bytes as f32 / t.total as f32 } else { 0.0 };
            (ink(), paper(), format!("{:.0}%", p * 100.0))
        }
        TState::Done => (stripe(i), ink(), if t.youtube { "YT".into() } else { "OK".into() }),
        TState::Failed => (Color::TRANSPARENT, ink(), l.t("TRYING YT…", "PROBANDO YT…").into()),
        TState::YtSearch | TState::YtDownload => (yellow(), ink(), "→ YT".into()),
        TState::Missing => (Color::TRANSPARENT, muted(), l.t("MISSING", "FALTA").into()),
        TState::Have => (Color::TRANSPARENT, ink(), String::new()),
        TState::Queued => (Color::TRANSPARENT, ink(), l.t("AGAIN ↓", "DE NUEVO ↓").into()),
        TState::Skipped => (Color::TRANSPARENT, muted(), l.t("HAD IT", "YA ESTABA").into()),
    };
    let mut label = text(title.clone()).font(mono(12.5, 600.0)).color(fg).truncate().grow(1.0).min_w(0.0);
    if t.state == TState::Missing {
        label = label.strike(0, title.len(), Some(muted()));
    }
    let mut chip_el = text(chip).font(mono(11.0, 600.0)).color(fg).ml(8.0);
    if choosing || matches!(t.state, TState::Searching | TState::Failed | TState::YtSearch | TState::YtDownload) {
        chip_el = chip_el.cycle(BLINK);
    }
    let mut r = row().key(format!("t{i}")).w_full().h(24.0).pl(6.0).pr(6.0).center_y().fill(bg)
        .cursor(Cursor::Pointer).role("button").label(format!("{} details", t.title))
        .on_press_tx(open_on_press(format!("c{i}")))
        .motion(Spring::new(0.35, 0.1))
        .transition(Transition::offset(0.0, 12.0).combined(Transition::opacity()))
        .enter_delay((i as f32 * 0.018).min(1.2));
    if open {
        r = r.outline(2.0, 1.0, ink());
    }
    // A red fill that grows behind the title as bytes arrive. It is always there (empty when
    // not downloading), so the row keeps the same children in every state.
    let p = if t.state == TState::Downloading && t.total > 0 { t.bytes as f32 / t.total as f32 } else { 0.0 };
    r = r.child(el().key("fill").absolute(0.0, 0.0).h_full().w_frac(p).fill(if p > 0.0 { red() } else { Color::TRANSPARENT }).motion(Spring::new(0.5, 0.0)));
    r = r.child(text(format!("{:>2}.", i + 1)).key("num").font(mono(12.0, 500.0)).color(fg).w(36.0))
        .child(label.key("title"))
        .child(chip_el.key("chip"));
    if t.state == TState::Have {
        // The question, next to the song.
        let mini = |key: &str, label: &'static str, dark: bool| {
            let (bg, fg) = if dark { (ink(), paper()) } else { (yellow(), ink()) };
            row().key(key).h(18.0).px(6.0).ml(4.0).center().fill(bg).border(1.5, ink()).cursor(Cursor::Pointer).role("button").label(label)
                .hover(red())
                .child(text(label).font(mono(10.0, 600.0)).color(fg))
        };
        r = r.child(mini("again", l.t("AGAIN", "DE NUEVO"), false).on_press(move |s: &mut State| s.decide(i, true)))
            .child(mini("skip", l.t("SKIP", "SALTEAR"), true).on_press(move |s: &mut State| s.decide(i, false)));
    }
    r
}

// ------------------------------------------------------------------ details

const PANEL_H: f32 = 236.0;
/// The panel of a track with nothing to show yet (no file, no peer): just the names.
const PANEL_SMALL_H: f32 = 118.0;

fn panel_h(d: &Detail) -> f32 {
    if d.path.is_empty() && d.peer.is_empty() && d.size == 0 && !d.moved && d.source.is_empty() { PANEL_SMALL_H } else { PANEL_H }
}

/// The band under a row: closed (no height), or open with the details of `open`.
fn band(s: &State, key: String, open: Option<String>, width: f32) -> El {
    let detail = open.as_deref().and_then(|k| s.detail_of(k).map(|d| (k.to_string(), d)));
    // A track of this run also shows its Soulseek files, under the details.
    let files = detail.as_ref().and_then(|(k, _)| {
        let i = k.strip_prefix('c')?.parse::<usize>().ok()?;
        let t = s.tracks.get(i)?;
        (t.in_run || !t.cands.is_empty()).then(|| cand_list(s, i, t))
    });
    let files_h = files.as_ref().map_or(0.0, |(_, h)| *h);
    let mut b = el().key(key).w_full().h(detail.as_ref().map_or(0.0, |(_, d)| panel_h(d) + files_h + 10.0)).clip_rounded(0.0).motion(Spring::new(OPEN_SECS, 0.0));
    if let Some((k, d)) = detail {
        let mut c = col().key(format!("open-{k}")).w_full().child(detail_panel(s, &k, &d, width));
        if let Some((f, _)) = files {
            c = c.child(f);
        }
        b = b.child(c);
    }
    b
}

/// Rows of files shown at most.
const CAND_ROWS: usize = 8;
const CAND_ROW_H: f32 = 30.0;

/// "FLAC 16/44.1", "MP3 320", "M4A 256".
fn format_label(c: &crate::backend::Cand) -> String {
    let ext = c.ext.to_uppercase();
    if matches!(c.ext.as_str(), "flac" | "wav" | "aif" | "aiff" | "ape" | "wv" | "alac") {
        match (c.bit_depth, c.sample_rate) {
            (b, r) if b > 0 && r > 0 => format!("{ext} {b}/{}", fmt_khz(r)),
            (_, r) if r > 0 => format!("{ext} {}", fmt_khz(r)),
            _ => ext,
        }
    } else if c.bitrate > 0 {
        format!("{ext} {}", c.bitrate)
    } else {
        ext
    }
}

fn fmt_khz(hz: u32) -> String {
    let k = hz as f32 / 1000.0;
    if (k - k.round()).abs() < 0.05 { format!("{:.0}", k) } else { format!("{:.1}", k) }
}

/// The Soulseek files of track `i`: what tells if it is the right song (length, name) and if it
/// comes fast (free slot or queue, speed, time). Click one to download it. Returns the height.
fn cand_list(s: &State, i: usize, t: &Track) -> (El, f32) {
    let l = s.lang;
    let light = paper().alpha(0.55);
    let can_pick = t.in_run && !matches!(t.state, TState::Done | TState::Missing | TState::YtSearch | TState::YtDownload);
    let head_text = if t.cands.is_empty() {
        if t.settled || !matches!(t.state, TState::Pending | TState::Searching) { l.t("NO FILES ON SOULSEEK", "NO HAY ARCHIVOS EN SOULSEEK").to_string() } else { l.t("SEARCHING SOULSEEK…", "BUSCANDO EN SOULSEEK…").to_string() }
    } else {
        let n = t.cands.len();
        let more = if t.settled { String::new() } else { l.t(" · MORE ARE COMING…", " · LLEGAN MÁS…").to_string() };
        match l {
            Lang::En => format!("{n} FILE{} ON SOULSEEK, BEST FIRST{more}", if n == 1 { "" } else { "S" }),
            Lang::Es => format!("{n} ARCHIVO{} EN SOULSEEK, EL MEJOR PRIMERO{more}", if n == 1 { "" } else { "S" }),
        }
    };
    let mut head = row().key("cands-head").w_full().h(28.0).gap(14.0).center_y()
        .child(text(head_text).font(mono(11.0, 600.0)).color(light).grow(1.0).min_w(0.0).truncate());
    if can_pick && s.settings.fallback {
        head = head.child(text(l.t("USE YOUTUBE MUSIC →", "USAR YOUTUBE MUSIC →")).key("cand-yt").font(mono(11.0, 600.0)).color(paper()).cursor(Cursor::Pointer).role("button")
            .on_press(move |s: &mut State| s.pick(i, None)));
    }
    let mut list = col().key(format!("cands-{i}")).w_full().px(18.0).pb(12.0).fill(ink()).transition(Transition::none()).instant_changes().child(head);
    let rows = t.cands.len().min(CAND_ROWS);
    for (k, c) in t.cands.iter().take(CAND_ROWS).enumerate() {
        list = list.child(cand_row(s, i, t, k, c, can_pick));
    }
    let h = 28.0 + rows as f32 * CAND_ROW_H + 12.0;
    (list, h)
}

fn cand_row(s: &State, i: usize, t: &Track, k: usize, c: &crate::backend::Cand, can_pick: bool) -> El {
    let l = s.lang;
    let current = t.cand == Some(c.id);
    let green = hex("#5fdcb4").unwrap_or(Color::WHITE);
    // Green: the right song from a free peer. Yellow: the song, but a wait or a doubt. Red: maybe not the song.
    let dot = if c.failed { muted() } else if c.fit >= 90 && c.free { green } else if c.fit >= 75 { yellow() } else { red() };
    let fg = if c.failed { paper().alpha(0.35) } else { paper() };
    let dim = paper().alpha(if c.failed { 0.25 } else { 0.55 });
    let cell_text = |key: &str, v: String, w: f32, color: Color| text(v).key(key).font(mono(11.5, 600.0)).color(color).w(w).truncate();
    // The length, and how far it is from Spotify's.
    let len = if c.duration > 0 {
        let d = c.duration as i64 - t.duration as i64;
        if t.duration == 0 || d.abs() <= 2 { clock(c.duration as f64) } else { format!("{} {}{}S", clock(c.duration as f64), if d > 0 { "+" } else { "−" }, d.abs()) }
    } else {
        "—".into()
    };
    let len_color = if c.duration > 0 && t.duration > 0 && (c.duration as i64 - t.duration as i64).abs() > 5 { red() } else { fg };
    let wait = if c.free { l.t("FREE NOW", "LIBRE YA").to_string() } else { format!("{} {}", l.t("QUEUE", "COLA"), c.queue) };
    let eta = if c.eta > 0 { format!("~{}", clock(c.eta as f64)) } else { String::new() };
    let mut r = row().key(format!("cand-{}", c.id)).w_full().h(CAND_ROW_H).gap(12.0).center_y().px(6.0)
        .child(el().size(8.0, 8.0).radius(4.0).fill(dot))
        .child(cell_text("fmt", format_label(c), 92.0, fg))
        .child(cell_text("size", fmt_size(c.size), 66.0, fg))
        .child(cell_text("len", len, 92.0, len_color))
        .child(cell_text("wait", wait, 86.0, if c.free { fg } else { yellow() }))
        .child(cell_text("speed", if c.speed > 0 { fmt_speed(c.speed as f32) } else { "—".into() }, 78.0, fg))
        .child(cell_text("eta", eta, 52.0, dim))
        .child(text(c.user.to_uppercase()).key("user").font(mono(11.5, 600.0)).color(fg).w(130.0).truncate())
        .child(text(if c.folder.is_empty() { c.file.clone() } else { format!("{}  ·  {}", c.file, c.folder) }).key("file").font(mono(11.0, 400.0)).color(dim).truncate().grow(1.0).min_w(0.0));
    if k == 0 && !c.failed {
        r = r.child(row().px(5.0).py(1.0).fill(green).child(text(l.t("BEST", "MEJOR")).font(mono(10.0, 600.0)).color(ink())));
    }
    let action = if current {
        let label = match t.state {
            TState::Downloading => l.t("DOWNLOADING", "BAJANDO"),
            TState::Done => l.t("THIS ONE", "ESTE"),
            _ => l.t("ASKING…", "PIDIENDO…"),
        };
        row().key("act").h(20.0).px(8.0).center().fill(red()).child(text(label).font(mono(10.5, 600.0)).color(paper()))
    } else if c.failed {
        row().key("act").h(20.0).px(8.0).center().child(text(l.t("FAILED", "FALLÓ")).font(mono(10.5, 600.0)).color(paper().alpha(0.4)))
    } else if can_pick {
        row().key("act").h(20.0).px(8.0).center().border(1.5, paper()).child(text(l.t("PICK", "ELEGIR")).font(mono(10.5, 600.0)).color(paper()))
    } else {
        el().key("act")
    };
    r = r.child(action);
    if current {
        r = r.fill(paper().alpha(0.12));
    }
    if can_pick && !current && !c.failed {
        let id = c.id;
        r = r.cursor(Cursor::Pointer).role("button").label(format!("Download {} from {}", c.file, c.user)).hover(paper().alpha(0.08))
            .on_press(move |s: &mut State| s.pick(i, Some(id)));
    }
    r
}

fn detail_panel(s: &State, key: &str, d: &Detail, width: f32) -> El {
    let l = s.lang;
    let light = paper().alpha(0.55);

    // The cover, or a block in the stripe colour with the first letter.
    let ph = panel_h(d);
    let mut cover = el().key("cover").size(ph - 36.0, ph - 36.0).border(2.0, paper()).clip_rounded(0.0);
    let pic = if d.path.is_empty() && d.cover.is_empty() { Cover::None } else { s.media.cover(&d.path, &d.cover) };
    cover = match pic {
        Cover::Ready(px) => cover.pixels_fit(&px, SurfaceFit::Fill),
        Cover::Loading => cover.fill(paper().alpha(0.08)),
        Cover::None => {
            let i = d.title.bytes().map(|b| b as usize).sum::<usize>();
            let letter: String = d.title.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
            cover.fill(stripe(i)).center().child(text(letter).font(display(if ph < PANEL_H { 60.0 } else { 120.0 })).color(ink()))
        }
    };

    let mut fact_list: Vec<El> = Vec::new();
    let mut fact = |label: &str, value: String| {
        if !value.is_empty() {
            fact_list.push(col().key(format!("fact-{label}")).gap(3.0)
                .child(text(label.to_string()).font(mono(10.0, 600.0)).color(light))
                .child(text(value).font(mono(13.0, 600.0)).color(paper())));
        }
    };
    let source = match d.source.as_str() {
        "soulseek" => "SOULSEEK",
        "youtube" => "YOUTUBE MUSIC",
        _ => "",
    };
    fact(l.t("SOURCE", "FUENTE"), source.into());
    if d.source == "soulseek" {
        fact(l.t("FROM", "DE"), d.peer.to_uppercase());
    }
    fact(l.t("QUALITY", "CALIDAD"), if d.bitrate > 0 { format!("{} KBPS", d.bitrate) } else { String::new() });
    fact(l.t("SIZE", "TAMAÑO"), if d.size > 0 { fmt_size(d.size) } else { String::new() });
    fact(l.t("AVG SPEED", "VEL. MEDIA"), if d.speed() > 0.0 { fmt_speed(d.speed() as f32) } else { String::new() });
    fact(l.t("TOOK", "TARDÓ"), if d.took() > 0.0 { clock(d.took()) } else { String::new() });
    let mut facts = row().key("facts").gap(26.0).flex_wrap();
    for f in fact_list {
        facts = facts.child(f);
    }

    let sub = [d.artist.clone(), d.album.clone()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    let title_row = row().key("title-row").w_full().gap(14.0).center_y()
        .child(text(d.title.to_uppercase()).font(display(40.0)).color(paper()).truncate().shrink().min_w(0.0))
        .child(row().px(8.0).py(3.0).fill(if d.bad { red() } else { yellow() })
            .child(text(d.status.clone()).font(mono(11.0, 600.0)).color(ink())));

    let mut links = row().key("links").gap(22.0);
    if !d.path.is_empty() && !d.path.starts_with("demo://") && !d.preview {
        let p = d.path.clone();
        links = links.child(text(l.t("SHOW IN FOLDER ↗", "MOSTRAR EN LA CARPETA ↗")).key("reveal").font(mono(12.0, 600.0)).color(paper()).cursor(Cursor::Pointer).role("link")
            .on_press(move |_: &mut State| reveal(&p)));
    }
    if !d.url.is_empty() {
        let u = d.url.clone();
        links = links.child(text(l.t("OPEN ON YOUTUBE ↗", "ABRIR EN YOUTUBE ↗")).key("yt").font(mono(12.0, 600.0)).color(paper()).cursor(Cursor::Pointer).role("link")
            .on_press_tx(move |_: &mut State, tx: &mut Tx| {
                let _ = tx.open_url(&u);
            }));
    }

    let player_el = if d.path.is_empty() {
        text(if d.moved {
            l.t("THE FILE IS NOT IN THE FOLDER ANY MORE.", "EL ARCHIVO YA NO ESTÁ EN LA CARPETA.")
        } else if d.source == "youtube" {
            l.t("THE PLAYER SHOWS UP WHEN THE TRACK IS DOWNLOADED.", "EL REPRODUCTOR APARECE CUANDO LA CANCIÓN SE DESCARGA.")
        } else {
            l.t("YOU CAN LISTEN AS SOON AS IT STARTS TO DOWNLOAD.", "LA PODÉS ESCUCHAR APENAS EMPIECE A DESCARGARSE.")
        }).key("no-player").font(mono(12.0, 500.0)).color(light)
    } else {
        player(s, &d.path, d.duration as f64, width)
    };

    let small = ph < PANEL_H;
    let player_el = if small { el().key("no-player") } else { player_el };
    row().key(format!("panel-{key}")).w_full().h(ph).padding(18.0).gap(22.0).fill(ink())
        // Everything inside is there from the first frame; only the band's height moves.
        .transition(Transition::none())
        .instant_changes()
        .child(cover)
        .child(col().key("info").grow(1.0).min_w(0.0).h_full().gap(10.0)
            .child(col().key("names").w_full().gap(2.0).child(title_row)
                .child(text(sub.to_uppercase()).font(mono(12.0, 500.0)).color(light).truncate()))
            .child(facts)
            .child(el().grow(1.0))
            .child(player_el)
            .child(links))
}

/// The length of `path` in seconds: from the player, the waveform, or Spotify.
pub fn track_len(s: &State, path: &str, spotify: f64) -> f64 {
    let st = s.media.status();
    if st.path == path && st.dur > 0.0 {
        return st.dur;
    }
    if let Wave::Ready(_, d) = s.media.wave(path) {
        return d;
    }
    spotify
}

/// Play / pause, the time, and a waveform to scrub through.
fn player(s: &State, path: &str, spotify: f64, _width: f32) -> El {
    let st = s.media.status();
    let mine = st.path == path;
    // A preview: the file is still downloading. Only the part on disk plays.
    let grow = s.media.growing(path).filter(|g| !g.complete());
    let len = match grow {
        Some(g) if g.secs > 0.0 => g.secs,
        _ => track_len(s, path, spotify),
    };
    let ready = grow.map(|g| g.playable());
    let pos = match &s.scrub {
        Some((p, f)) if p == path => *f as f64 * len,
        _ if mine => st.pos,
        _ => 0.0,
    };
    let frac = if len > 0.0 { (pos / len).clamp(0.0, 1.0) as f32 } else { 0.0 };
    // The part that can play, 0 to 1.
    let have = match ready {
        Some(r) if len > 0.0 => (r / len).clamp(0.0, 1.0) as f32,
        Some(_) => 0.0,
        None => 1.0,
    };
    let playing = mine && (st.playing || st.buffering);
    let p = path.to_string();
    let button = row().key("play").size(46.0, 46.0).radius(23.0).center().fill(paper()).cursor(Cursor::Pointer).role("button")
        .label(if playing { "Pause" } else { "Play" })
        .hover(red())
        .press_scale(0.94)
        .on_press_tx(move |s: &mut State, tx: &mut Tx| {
            if s.demo || p.starts_with("demo://") {
                return;
            }
            s.media.toggle(&p);
            tx.wake_after(std::time::Duration::from_millis(100));
        })
        .child(text(if playing { "❚❚" } else { "▶" }).font(mono(if playing { 14.0 } else { 18.0 }, 600.0)).color(ink()));

    let mut bars = row().key("wave").h(52.0).grow(1.0).min_w(0.0).gap(2.0).items_center().cursor(Cursor::Pointer);
    match s.media.wave(path) {
        Wave::Ready(peaks, _) => {
            for (k, v) in peaks.iter().enumerate() {
                let x = (k as f32 + 0.5) / peaks.len() as f32;
                let on = x <= frac;
                // Not downloaded yet: a low, faint line.
                let (h, color) = if x > have { (3.0, paper().alpha(0.14)) } else { (4.0 + v * 46.0, if on { red() } else { paper().alpha(0.4) }) };
                bars = bars.child(el().key(format!("b{k}")).grow(1.0).h(h).radius(1.0).fill(color));
            }
        }
        Wave::Loading if grow.is_some() => {
            // The start of a preview: the downloaded part as a bar.
            bars = bars.child(el().key("got").h(6.0).w_frac(have.max(0.01)).radius(3.0).fill(paper().alpha(0.4)))
                .child(el().key("left").h(2.0).grow(1.0).fill(paper().alpha(0.14)));
        }
        Wave::Loading => bars = bars.child(text(s.lang.t("READING THE WAVEFORM…", "LEYENDO LA ONDA…")).font(mono(11.0, 500.0)).color(paper().alpha(0.5)).cycle(BLINK)),
        Wave::Failed => bars = bars.child(el().grow(1.0).h(2.0).fill(paper().alpha(0.4))),
    }
    let p = path.to_string();
    bars = bars.on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| {
        if s.demo || p.starts_with("demo://") {
            return;
        }
        let f = (d.x / d.w.max(1.0)).clamp(0.0, 1.0);
        match d.phase {
            DragPhase::Down | DragPhase::Move => {
                // Only to where it has downloaded.
                let f = match s.media.growing(&p).filter(|g| !g.complete()) {
                    Some(g) if g.secs > 0.0 => f.min((g.playable() / g.secs) as f32),
                    _ => f,
                };
                s.scrub = Some((p.clone(), f));
            }
            DragPhase::Up => {
                let f = s.scrub.take().map_or(f, |(_, f)| f);
                let len = match s.media.growing(&p).filter(|g| !g.complete()) {
                    Some(g) if g.secs > 0.0 => g.secs,
                    _ => track_len(s, &p, spotify),
                };
                if len > 0.0 {
                    s.media.seek(&p, f as f64 * len);
                    tx.wake_after(std::time::Duration::from_millis(100));
                }
            }
        }
    });
    let mut r = row().key("player").w_full().gap(14.0).center_y()
        .child(button)
        .child(text(clock(pos)).key("pos").font(mono(12.0, 600.0)).color(paper()).w(48.0))
        .child(bars)
        .child(text(clock(len)).key("len").font(mono(12.0, 600.0)).color(paper()).w(48.0));
    if mine && !st.error.is_empty() {
        r = r.child(text(st.error.to_uppercase()).font(mono(11.0, 600.0)).color(red()).truncate().max_w(220.0));
    } else if let Some(rd) = ready {
        let l = s.lang;
        let note = if mine && st.buffering { l.t("WAITING FOR MORE…", "ESPERANDO MÁS…").to_string() } else { format!("{} {}", clock(rd), l.t("READY", "LISTO")) };
        r = r.child(text(note).key("ready").font(mono(11.0, 600.0)).color(yellow()).w(130.0).truncate());
    }
    r
}

/// Shows the file in Finder / Explorer.
pub fn reveal(path: &str) {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{path}"));
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(std::path::Path::new(path).parent().unwrap_or(std::path::Path::new(".")));
        c
    };
    let _ = cmd.spawn();
}

// ------------------------------------------------------------------ 3. poster and history

fn poster(s: &State, size: [f32; 2]) -> El {
    let l = s.lang;
    let n = s.tracks.len().max(1);
    let has_history = s.history.sessions.iter().any(|h| h.id != s.session_id);
    // With a history, leave room to see where it starts.
    let avail = (size[1] - 54.0 - 70.0) * if has_history { 0.7 } else { 1.0 };
    // The first stripes are the biggest; each one is a little smaller than the one before,
    // and together they fill the window (or scroll, for very long lists).
    let r = (1.0f32 / 3.0).powf(1.0 / (n as f32 - 1.0).max(1.0));
    let sum: f32 = (0..n).map(|i| r.powi(i as i32)).sum();
    let mut stripes = col().key("stripes").w_full();
    for (i, t) in s.tracks.iter().enumerate() {
        // Few tracks get bigger stripes.
        let h = (avail * r.powi(i as i32) / sum).clamp(13.0, if n <= 3 { 220.0 } else { 140.0 }).floor();
        let fs = (h * 0.9).floor();
        let title = t.title.to_uppercase();
        let had = t.state == TState::Skipped || t.state == TState::Have;
        let missing = !had && t.state != TState::Done;
        let mut label = text(title.clone()).font(display(fs)).color(ink()).truncate().shrink().min_w(0.0);
        if missing {
            label = label.strike(0, title.len(), Some(ink()));
        }
        let key = format!("c{i}");
        let fill = if missing { c("#bdbdbb") } else if had { paper() } else { stripe(i) };
        let mut row_el = row().key(format!("p{i}")).w_full().h(h).px(28.0).gap(14.0).center_y().clip_rounded(0.0)
            .fill(fill)
            .cursor(Cursor::Pointer).role("button").label(format!("{} details", t.title))
            .on_press_tx(open_on_press(key.clone()))
            .transition(Transition::offset(-60.0, 0.0).combined(Transition::opacity()))
            .enter_delay((i as f32 * 0.035).min(1.6))
            .child(label)
            .child(el().grow(1.0));
        if had {
            row_el = row_el.border(1.0, ink().alpha(0.15));
        }
        let tag = |t: &str| row().px(6.0).py(2.0).fill(ink()).child(text(t.to_string()).font(mono((fs * 0.4).clamp(10.0, 18.0), 600.0)).color(paper()));
        if t.youtube && t.state == TState::Done {
            row_el = row_el.child(tag("YT"));
        }
        if had {
            row_el = row_el.child(tag(l.t("ALREADY HAD", "YA ESTABA")));
        }
        if missing {
            row_el = row_el.child(text(if s.stopped_early { l.t("NOT DOWNLOADED", "NO DESCARGADA") } else { l.t("NOT FOUND", "NO ENCONTRADA") }).font(mono((fs * 0.35).clamp(10.0, 16.0), 600.0)).color(ink()));
        }
        let open = (s.open.as_deref() == Some(key.as_str())).then_some(key);
        stripes = stripes.child(row_el).child(band(s, format!("pband-{i}"), open, size[0]));
    }
    let mb = s.tracks.iter().filter(|t| t.state == TState::Done && !t.youtube).map(|t| t.total).sum::<u64>() as f32 / 1_000_000.0;
    let yt = s.tracks.iter().filter(|t| t.youtube && t.state == TState::Done).count();
    let missing = s.count(TState::Missing);
    let had = s.count(TState::Skipped);
    let mut stats = l.tracks(s.count(TState::Done));
    if mb > 0.0 {
        stats += &format!(" · {mb:.0} MB {}", l.t("FROM SOULSEEK", "DE SOULSEEK"));
    }
    if yt > 0 {
        stats += &format!(" · {yt} {}", l.t("FROM YOUTUBE", "DE YOUTUBE"));
    }
    if had > 0 {
        stats += &format!(" · {had} {}", l.t("ALREADY HAD", "YA ESTABAN"));
    }
    if missing > 0 {
        stats += &format!(" · {missing} {}", if s.stopped_early { l.t("NOT DOWNLOADED (STOPPED)", "SIN DESCARGAR (FRENADO)") } else { l.t("MISSING", "FALTAN") });
    }
    let top = row().key("poster-top").w_full().h(54.0).px(24.0).center_y().justify_between()
        .child(text(format!("{} {}", l.t("DONE.", "LISTO."), s.name.to_uppercase())).font(mono(15.0, 600.0)).color(ink()).truncate().shrink().min_w(0.0))
        .child(link_text("new-link", l.t("NEW DOWNLOAD →", "NUEVA DESCARGA →")).on_press(|s: &mut State| s.new_link()));
    let bottom = row().key("poster-bottom").w_full().h(70.0).px(24.0).gap(20.0).center()
        .child(button("open-folder", l.t("OPEN FOLDER →", "ABRIR CARPETA →"), true).on_press(|s: &mut State| open_folder(if s.out_dir.is_empty() { &s.settings.dir } else { &s.out_dir })))
        .child(text(stats).key("stats").font(mono(13.0, 600.0)).color(ink()));
    let mut list = col().key("poster-list").w_full().child(stripes);
    if has_history {
        list = list.child(history_list(s, &s.session_id, size[0]));
    }
    col().key("poster").w_full().h_full()
        .child(top)
        .child(col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
        .child(bottom)
}

/// The history screen, from the paste screen.
fn history_screen(s: &State, size: [f32; 2]) -> El {
    let l = s.lang;
    let n: usize = s.history.sessions.iter().map(|h| h.tracks.iter().filter(|t| t.state == "done").count()).sum();
    let top = row().key("history-top").w_full().h(54.0).px(24.0).center_y().justify_between()
        .child(text(format!("{} · {}", l.t("HISTORY", "HISTORIAL"), l.tracks(n))).font(mono(15.0, 600.0)).color(ink()))
        .child(link_text("history-back", l.t("← BACK", "← VOLVER")).on_press(|s: &mut State| {
            s.show_history = false;
            s.open = None;
        }));
    let body = if s.history.sessions.is_empty() {
        col().w_full().grow(1.0).center().child(text(l.t("NOTHING DOWNLOADED YET", "TODAVÍA NO DESCARGASTE NADA")).font(mono(13.0, 600.0)).color(muted()))
    } else {
        col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(history_list(s, "", size[0]))
    };
    col().key("history").w_full().h_full().child(top).child(el().w_full().h(2.0).fill(ink())).child(body)
}

/// "7 NOV 2026".
fn date(l: Lang, unix: f64) -> String {
    let (y, m, d) = crate::model::civil((unix as i64).div_euclid(86_400));
    format!("{d} {} {y}", l.month(m))
}

/// Every earlier session but `except`: a divider, then each session's tracks.
fn history_list(s: &State, except: &str, width: f32) -> El {
    let l = s.lang;
    let mut list = col().key("history-list").w_full().pt(26.0).pb(20.0);
    if !except.is_empty() {
        list = list.child(row().key("divider").w_full().px(24.0).pb(18.0).gap(14.0).center_y()
            .child(el().h(2.0).grow(1.0).fill(ink()))
            .child(text(l.t("HISTORY", "HISTORIAL")).font(display(30.0)).color(ink()))
            .child(el().h(2.0).grow(1.0).fill(ink())));
    }
    for sess in s.history.sessions.iter().filter(|h| h.id != except) {
        let done = sess.tracks.iter().filter(|t| t.state == "done").count();
        list = list.child(row().key(format!("sess-{}", sess.id)).w_full().h(40.0).px(24.0).gap(12.0).center_y()
            .child(text(sess.name.to_uppercase()).font(mono(14.0, 600.0)).color(ink()).truncate().shrink().min_w(0.0))
            .child(text(format!("{} · {} · {done}/{}", l.kind(&sess.kind), date(l, sess.started), l.tracks(sess.tracks.len()))).font(mono(11.0, 600.0)).color(muted()))
            .child(el().grow(1.0))
            .child(link_text(&format!("folder-{}", sess.id), l.t("FOLDER ↗", "CARPETA ↗")).on_press({
                let dir = sess.dir.clone();
                move |_: &mut State| open_folder(&dir)
            })));
        for (j, t) in sess.tracks.iter().enumerate() {
            let key = format!("h{}:{j}", sess.id);
            let gone = t.state != "done" || !std::path::Path::new(&t.path).is_file();
            let title = t.title.to_uppercase();
            let mut label = text(title.clone()).font(mono(13.0, 600.0)).color(if gone { muted() } else { ink() }).truncate().shrink().min_w(0.0);
            if t.state != "done" {
                label = label.strike(0, title.len(), Some(muted()));
            }
            let note = if t.state != "done" {
                l.t("NOT FOUND", "NO ENCONTRADA")
            } else if gone {
                l.t("FILE MOVED", "SE MOVIÓ")
            } else if t.source == "youtube" {
                "YT"
            } else {
                ""
            };
            let open = s.open.as_deref() == Some(key.as_str());
            let mut r = row().key(format!("hrow-{key}")).w_full().h(30.0).px(24.0).gap(12.0).center_y()
                .cursor(Cursor::Pointer).role("button").label(format!("{} details", t.title))
                .on_press_tx(open_on_press(key.clone()))
                .hover(ink().alpha(0.06))
                // A stripe of the poster colour, gray when the file is gone.
                .child(el().size(10.0, 18.0).fill(if gone { c("#bdbdbb") } else { stripe(j) }))
                .child(label)
                .child(text(t.artist.to_uppercase()).font(mono(11.0, 500.0)).color(muted()).truncate().shrink().min_w(0.0))
                .child(el().grow(1.0))
                .child(text(note).font(mono(11.0, 600.0)).color(muted()));
            if open {
                r = r.fill(ink().alpha(0.08));
            }
            list = list.child(r).child(band(s, format!("hband-{key}"), open.then_some(key.clone()), width));
        }
        list = list.child(el().h(14.0));
    }
    list
}

pub fn open_folder(path: &str) {
    let cmd = if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = std::process::Command::new(cmd).arg(path).spawn();
}

// ------------------------------------------------------------------ settings

fn settings_panel(s: &State) -> El {
    let l = s.lang;
    let label = |t: &str| text(t.to_string()).font(mono(11.0, 600.0)).color(muted());
    let field = |key: &str, edit: &rune::TextEdit| text_input(edit, mono(15.0, 500.0)).key(key).single_line().w_full().color(ink()).caret(ink(), red().alpha(0.25));
    let boxed = |e: El| row().w_full().h(42.0).px(12.0).center_y().border(2.0, ink()).fill(Color::WHITE).child(e);
    let choice = |key: String, label: &str, on: bool| {
        row().key(key).h(40.0).px(14.0).center().border(2.0, ink()).cursor(Cursor::Pointer)
            .fill(if on { ink() } else { Color::TRANSPARENT })
            .child(text(label.to_string()).font(mono(14.0, 600.0)).color(if on { paper() } else { ink() }))
    };
    let mut quality = row().gap(0.0);
    for b in ["320", "256", "192", "128"] {
        quality = quality.child(choice(format!("q{b}"), b, s.settings.bitrate == b).w(70.0).on_press(move |s: &mut State| s.settings.bitrate = b.into()));
    }
    let mut lang = row().gap(0.0);
    for (k, name) in [("auto", l.t("SYSTEM", "SISTEMA")), ("en", "ENGLISH"), ("es", "ESPAÑOL")] {
        lang = lang.child(choice(format!("lang-{k}"), name, s.settings.lang == k).on_press(move |s: &mut State| s.set_lang(k)));
    }
    let fb = s.settings.fallback;
    let panel = col().key("settings").w(600.0).padding(28.0).gap(12.0).fill(paper()).border(3.0, ink())
        .shadow(18.0, 0.0, ink())
        .on_press(|_: &mut State| {})
        .child(text(l.t("SETTINGS", "AJUSTES")).font(display(54.0)).color(ink()))
        .child(row().key("account-title").gap(10.0).center_y()
            .child(label(l.t("SOULSEEK ACCOUNT", "CUENTA DE SOULSEEK")))
            .child(if s.account == Account::None { ping("account-ping") } else { el().key("account-ping") }))
        .child(login_form(s))
        .child(label(l.t("DOWNLOAD FOLDER", "CARPETA DE DESCARGAS")))
        .child(row().w_full().gap(10.0)
            .child(boxed(field("dir", &s.dir_edit).on_edit(|s: &mut State| &mut s.dir_edit).on_submit(|s: &mut State, _: &mut Tx| s.close_settings())).shrink().grow(1.0).min_w(0.0))
            .child(button("choose-dir", l.t("CHOOSE…", "ELEGIR…"), false).on_press_tx(|s: &mut State, tx: &mut Tx| {
                let options = FileDialogOptions { title: s.lang.t("Download folder", "Carpeta de descargas").into(), directory: Some(s.settings.dir.clone().into()), ..Default::default() };
                tx.open_folder(options, |s: &mut State, result, _| {
                    if let Ok(Some(p)) = result {
                        s.dir_edit.replace_text(p.to_string_lossy().into_owned());
                    }
                });
            })))
        .child(row().w_full().gap(28.0)
            .child(col().gap(8.0).child(label(l.t("MP3 QUALITY (KBPS)", "CALIDAD DEL MP3 (KBPS)"))).child(quality))
            .child(col().gap(8.0).child(label(l.t("LANGUAGE", "IDIOMA"))).child(lang)))
        .child(label(l.t("WHICH FILE TO DOWNLOAD", "QUÉ ARCHIVO DESCARGAR")))
        .child(row().gap(0.0)
            .child(choice("files-auto".into(), l.t("THE BEST ONE, BY ITSELF", "EL MEJOR, SOLO"), !s.settings.manual).on_press(|s: &mut State| s.settings.manual = false))
            .child(choice("files-manual".into(), l.t("I PICK FOR EACH SONG", "ELIJO YO EN CADA CANCIÓN"), s.settings.manual).on_press(|s: &mut State| s.settings.manual = true)))
        .child(label(l.t("WHEN SOULSEEK CANNOT FIND A TRACK", "SI SOULSEEK NO ENCUENTRA UNA CANCIÓN")))
        .child(row().key("fallback").gap(10.0).center_y().cursor(Cursor::Pointer).on_press(|s: &mut State| s.settings.fallback = !s.settings.fallback)
            .child(row().size(22.0, 22.0).center().border(2.0, ink()).fill(if fb { ink() } else { Color::TRANSPARENT })
                .child(text(if fb { "✓" } else { "" }).font(mono(13.0, 600.0)).color(paper())))
            .child(text(l.t("TRY YOUTUBE MUSIC", "PROBAR CON YOUTUBE MUSIC")).font(mono(14.0, 600.0)).color(ink())))
        .child(el().h(6.0))
        .child(row().w_full().justify_between().center_y()
            .child(text(l.t("ENTER SAVES · ESC CLOSES", "ENTER GUARDA · ESC CIERRA")).font(mono(11.0, 500.0)).color(muted()))
            .child(button("save-settings", l.t("DONE", "LISTO"), true).on_press(|s: &mut State| s.close_settings())));
    col().key("settings-layer").absolute(0.0, 0.0).w_full().h_full().center()
        .child(el().key("scrim").absolute(0.0, 0.0).w_full().h_full().fill(ink().alpha(0.35)).on_press(|s: &mut State| s.close_settings()))
        .child(panel)
        .transition(Transition::opacity())
}

/// The Soulseek account: who is signed in, or the form to sign in (which also creates the
/// account, as Soulseek has no sign-up page).
fn login_form(s: &State) -> El {
    let l = s.lang;
    match &s.account {
        Account::App(name, _) => {
            return row().key("signed-in").w_full().h(42.0).gap(12.0).center_y()
                .child(el().size(10.0, 10.0).radius(5.0).fill(c("#2e9e4f")))
                .child(text(format!("{} {}", l.t("SIGNED IN AS", "SESIÓN INICIADA COMO"), name.to_uppercase())).font(mono(14.0, 600.0)).color(ink()).grow(1.0))
                .child(link_text("sign-out", l.t("SIGN OUT", "CERRAR SESIÓN")).on_press(|s: &mut State| s.sign_out()));
        }
        Account::Conf(name) => {
            return col().key("signed-in").w_full().gap(4.0)
                .child(row().gap(12.0).center_y()
                    .child(el().size(10.0, 10.0).radius(5.0).fill(c("#2e9e4f")))
                    .child(text(format!("{} {}", l.t("USING THE ACCOUNT IN sockseek.conf:", "USANDO LA CUENTA DE sockseek.conf:"), name.to_uppercase())).font(mono(14.0, 600.0)).color(ink())));
        }
        Account::None => {}
    }
    let field = |key: &str, edit: &rune::TextEdit| {
        text_input(edit, mono(15.0, 500.0)).key(key).single_line().w_full().color(ink()).caret(ink(), red().alpha(0.25))
            .on_submit(|s: &mut State, _: &mut Tx| s.sign_in())
    };
    let boxed = |e: El| row().w_full().h(42.0).px(12.0).center_y().border(2.0, ink()).fill(Color::WHITE).child(e);
    let mut f = col().key("login-form").w_full().gap(10.0)
        .child(text(l.t(
            "Soulseek is a free network where people share music. Spotseek downloads from it with your account. No account yet? Pick a name and a password here: if the name is free, the account is created.",
            "Soulseek es una red gratis donde la gente comparte música. Spotseek descarga de ahí con tu cuenta. ¿Todavía no tenés? Elegí un nombre y una contraseña acá: si el nombre está libre, la cuenta se crea.",
        )).key("login-about").font(mono(12.0, 500.0)).color(ink()).wrap())
        .child(text(l.t(
            "Save the password somewhere: Soulseek cannot reset it.",
            "Guardá la contraseña en algún lado: Soulseek no la puede recuperar.",
        )).key("login-save").font(mono(12.0, 600.0)).color(ink()).wrap())
        .child(row().w_full().gap(10.0)
            .child(boxed(field("login-user", &s.login_user).placeholder(l.t("name", "nombre"), muted()).on_edit(|s: &mut State| &mut s.login_user)).grow(1.0).min_w(0.0))
            .child(boxed(field("login-pass", &s.login_pass).secure().placeholder(l.t("password", "contraseña"), muted()).on_edit(|s: &mut State| &mut s.login_pass)).grow(1.0).min_w(0.0)));
    if !s.login_error.is_empty() {
        f = f.child(text(s.login_error.clone()).key("login-error").font(mono(12.0, 600.0)).color(red()).wrap());
    }
    let go = button("sign-in", if s.login_busy { l.t("SIGNING IN…", "ENTRANDO…") } else { l.t("SIGN IN / CREATE", "ENTRAR / CREAR") }, true)
        .on_press(|s: &mut State| s.sign_in());
    f.child(row().w_full().gap(14.0).center_y()
        .child(go)
        .child(text(l.t("One account works on one computer at a time.", "Una cuenta funciona en una sola computadora a la vez.")).font(mono(11.0, 500.0)).color(muted()).wrap().shrink().min_w(0.0)))
}

/// The window that asks for the Soulseek account at launch. It can be skipped.
fn login_window(s: &State) -> El {
    let l = s.lang;
    let panel = col().key("login").w(600.0).padding(28.0).gap(14.0).fill(paper()).border(3.0, ink())
        .shadow(18.0, 0.0, ink())
        .on_press(|_: &mut State| {})
        .child(row().gap(16.0).center_y()
            .child(text("SOULSEEK").font(display(54.0)).color(ink()))
            .child(if s.account == Account::None { ping("login-ping") } else { el().key("login-ping") }))
        .child(login_form(s))
        .child(el().w_full().h(2.0).fill(ink().alpha(0.15)))
        .child(row().w_full().gap(14.0).center_y()
            .child(text(l.t(
                "Without an account everything comes from YouTube Music: slower, and the quality is lower.",
                "Sin cuenta, todo se baja de YouTube Music: es más lento y la calidad es menor.",
            )).font(mono(11.0, 500.0)).color(muted()).wrap().shrink().grow(1.0).min_w(0.0))
            .child(link_text("skip-login", l.t("SKIP →", "SALTEAR →")).on_press(|s: &mut State| s.login_open = false)));
    col().key("login-layer").absolute(0.0, 0.0).w_full().h_full().center()
        .child(el().key("login-scrim").absolute(0.0, 0.0).w_full().h_full().fill(ink().alpha(0.35)))
        .child(panel)
        .transition(Transition::opacity())
}
