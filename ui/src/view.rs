//! The screens: paste a link, watch it work, look at the poster and the history. Every track
//! opens a details panel with its cover, where it came from, and a player.

use rune::color::hex;
use rune::{
    Color, Curve, Cursor, Cx, Drag, El, Easing, Family, FileDialogOptions, Phase as DragPhase, ScrollSpec, Spring, SurfaceFit, TextStyle, Transition, Tx, col, el,
    row, text, text_input,
};
use rune_motion::Cycle;

use crate::gfx;
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
        "THAT IS NOT A SPOTIFY LINK. PLAYLISTS, ALBUMS AND TRACKS WORK.".into()
    } else if ok {
        "PRESS ENTER ↵ TO DOWNLOAD".into()
    } else {
        "PASTE WITH ⌘V ANYWHERE · PLAYLIST, ALBUM OR TRACK".into()
    };
    let hint_color = if has_text && !ok { red() } else { muted() };
    let footer = row().w_full().h(40.0).center_y().justify_between()
        .child(text(format!("{} · MP3 {}", s.settings.short_dir(), s.settings.bitrate)).key("where").font(mono(14.0, 500.0)).color(ink())
            .cursor(Cursor::Pointer).on_press(|s: &mut State| s.settings_open = true))
        .child(row().gap(28.0)
            .child(link_text("classic", "CLASSIC WINDOW").on_press(|s: &mut State| {
                if let Err(e) = crate::backend::open_classic() {
                    s.notice = e.to_uppercase();
                }
            }))
            .child(link_text("open-history", "HISTORY").on_press(|s: &mut State| {
                s.show_history = true;
                s.open = None;
            }))
            .child(link_text("open-settings", "SETTINGS").on_press(|s: &mut State| s.settings_open = true)));
    col().key("idle").w_full().h_full().px(44.0).pt(36.0).pb(14.0).gap(18.0)
        .child(headline(s, "Paste a Spotify link").grow(1.0).min_h(160.0))
        .child(row().w_full().center().gap(18.0).pt(10.0)
            .child(col().w(field_w).gap(6.0).child(field).child(underline))
            .child(go))
        .child(row().w_full().center().child(text(hint).key("hint").font(mono(12.0, 500.0)).color(hint_color)))
        .child(el().h(18.0))
        .child(footer)
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
    if s.phase == Phase::Failed {
        return "FAILED";
    }
    let running = s.tracks.iter().any(|t| t.in_run);
    if !s.tracks.is_empty() && !running && s.asking() > 0 {
        return "ALREADY IN YOUR FOLDER";
    }
    match s.stage.as_str() {
        "spotify" => "READING SPOTIFY",
        "soulseek" if s.downloading() => "DOWNLOADING FROM SOULSEEK",
        "soulseek" => "SEARCHING SOULSEEK",
        "youtube" => "TRYING YOUTUBE MUSIC",
        "convert" => "CONVERTING TO MP3",
        "done" => "FINISHING",
        _ => "CONNECTING",
    }
}

fn stat(key: &str, label: &str, value: String) -> El {
    col().key(key).px(18.0).gap(3.0).items_end()
        .child(text(label).font(mono(11.0, 600.0)).color(muted()))
        .child(text(value).font(mono(15.0, 600.0)).color(ink()))
}

/// "FROM ZWEM ⟶", "FROM ZWEM +2 PEERS ⟶".
fn peer_label(s: &State) -> String {
    let peers = s.peers();
    match peers.len() {
        0 if s.stage == "convert" => format!("CONVERTING {}/{}", s.convert.0, s.convert.1),
        0 => "WAITING FOR PEERS".into(),
        1 => format!("FROM {} ⟶", peers[0].to_uppercase()),
        n => format!("FROM {} +{} PEERS ⟶", peers[0].to_uppercase(), n - 1),
    }
}

fn working(s: &State, size: [f32; 2]) -> El {
    let n = s.tracks.len();
    let coll = if n == 0 { "…".to_string() } else { format!("{}: {} · {} TRACKS", s.kind.to_uppercase(), s.name.to_uppercase(), n) };
    let header = row().key("header").w_full().h(68.0).px(24.0).center_y().justify_between()
        .child(col().gap(4.0).shrink().min_w(0.0)
            .child(row().gap(8.0).center_y()
                .child(el().size(9.0, 9.0).radius(5.0).fill(if s.phase == Phase::Failed { red() } else { ink() }).cycle(BLINK))
                .child(text(status(s)).key("status").font(mono(15.0, 600.0)).color(ink())))
            .child(text(s.running_link.clone()).font(mono(12.0, 400.0)).color(muted()).truncate()))
        .child(row().center_y()
            .child(stat("stat-coll", "COLLECTION", coll))
            .child(el().w(2.0).h(34.0).fill(ink()))
            .child(stat("stat-progress", "PROGRESS", format!("{}/{}", s.finished(), n)))
            .child(el().w(2.0).h(34.0).fill(ink()))
            .child(stat("stat-speed", "SPEED", fmt_speed(s.speed))));
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
            .child(text(format!("{} DONE · {} LEFT", s.count(TState::Done), left)).font(mono(12.0, 600.0)).color(ink())));

    let mut below = col().key("below").w_full().grow(1.0).min_h(0.0).gap(10.0);
    if s.phase == Phase::Failed {
        below = below.child(col().key("failed").w_full().gap(16.0).pt(10.0)
            .child(text(if s.error.is_empty() { "Something went wrong.".into() } else { s.error.clone() }).font(mono(13.0, 500.0)).color(red()).wrap())
            .child(row().gap(14.0)
                .child(button("retry", "TRY AGAIN", true).on_press(|s: &mut State| s.start()))
                .child(button("new-link", "NEW LINK", false).on_press(|s: &mut State| s.new_link()))));
    } else {
        if s.asking() > 0 {
            below = below.child(ask_banner(s));
        }
        below = below.child(track_grid(s, size[0] - 56.0));
    }

    let footer = row().key("footer").w_full().h(46.0).px(24.0).gap(18.0).center_y()
        .child(row().key("stop").gap(8.0).center_y().cursor(Cursor::Pointer).role("button").label("Stop")
            .on_press(|s: &mut State| if s.phase == Phase::Running { s.stop() } else { s.new_link() })
            .child(el().size(16.0, 16.0).radius(8.0).border(2.0, ink()))
            .child(text(if s.phase == Phase::Running { "STOP" } else { "BACK" }).font(mono(13.0, 600.0)).color(ink())))
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
    let n = s.asking();
    let what = if n == 1 { "1 TRACK IS".to_string() } else { format!("{n} TRACKS ARE") };
    let mut msg = format!("{what} ALREADY IN {}", s.settings.short_dir().to_uppercase());
    let left = s.countdown_left();
    if let Some(l) = left {
        msg += &format!(" · SKIPPING IN {}", l.ceil() as u32);
    } else {
        msg += " · THEY ARE SKIPPED WHEN THE REST IS DONE";
    }
    let frac = left.zip(s.countdown).map_or(1.0, |(l, (_, len))| (l / len) as f32);
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
            .child(small("ask-again", "DOWNLOAD AGAIN", false).on_press(|s: &mut State| s.decide_all(true)))
            .child(small("ask-skip", "SKIP", true).on_press(|s: &mut State| s.decide_all(false))))
        // The time left, as a bar that runs out.
        .child(el().key("ask-time").h(4.0).w_frac(frac).fill(ink()).motion(Spring::new(0.15, 0.0)))
}

/// Tracks in rows, left to right. Under each row is a band that opens to show the details of
/// a track in that row.
fn track_grid(s: &State, width: f32) -> El {
    let n = s.tracks.len();
    if n == 0 {
        return col().key("tracks").w_full().grow(1.0).center()
            .child(text("READING THE TRACK LIST…").font(mono(13.0, 600.0)).color(muted()).cycle(BLINK));
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
            line = line.child(el().key(format!("slot-{i}")).w(cw).pr(10.0).child(cell(i, &s.tracks[i], s.open.as_deref() == Some(format!("c{i}").as_str()))));
        }
        grid = grid.child(line).child(band(s, format!("band-{r}"), open, width - 10.0));
    }
    col().key("tracks").w_full().grow(1.0).min_h(0.0).pb(10.0).scroll(ScrollSpec { stick_bottom: false }).child(grid)
}

/// Accordion motion: fast at the start, a long soft landing.
const EASE_OUT: Easing = Easing::cubic(0.23, 1.0, 0.32, 1.0);
const OPEN_SECS: f32 = 0.34;

fn open_on_press(key: String) -> impl Fn(&mut State, &mut Tx) + 'static {
    move |s: &mut State, tx: &mut Tx| {
        tx.motion_curve(Curve::tween(OPEN_SECS, EASE_OUT));
        s.toggle_open(key.clone());
    }
}

fn cell(i: usize, t: &Track, open: bool) -> El {
    let title = t.title.to_uppercase();
    let (bg, fg, chip): (Color, Color, String) = match t.state {
        TState::Pending => (Color::TRANSPARENT, muted(), String::new()),
        TState::Searching => (Color::TRANSPARENT, ink(), "…".into()),
        TState::Downloading => {
            let p = if t.total > 0 { t.bytes as f32 / t.total as f32 } else { 0.0 };
            (ink(), paper(), format!("{:.0}%", p * 100.0))
        }
        TState::Done => (stripe(i), ink(), if t.youtube { "YT".into() } else { "OK".into() }),
        TState::Failed => (Color::TRANSPARENT, ink(), "TRYING YT…".into()),
        TState::YtSearch | TState::YtDownload => (yellow(), ink(), "→ YT".into()),
        TState::Missing => (Color::TRANSPARENT, muted(), "MISSING".into()),
        TState::Have => (Color::TRANSPARENT, ink(), String::new()),
        TState::Queued => (Color::TRANSPARENT, ink(), "AGAIN ↓".into()),
        TState::Skipped => (Color::TRANSPARENT, muted(), "HAD IT".into()),
    };
    let mut label = text(title.clone()).font(mono(12.5, 600.0)).color(fg).truncate().grow(1.0).min_w(0.0);
    if t.state == TState::Missing {
        label = label.strike(0, title.len(), Some(muted()));
    }
    let mut chip_el = text(chip).font(mono(11.0, 600.0)).color(fg).ml(8.0);
    if matches!(t.state, TState::Searching | TState::Failed | TState::YtSearch | TState::YtDownload) {
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
        r = r.child(mini("again", "AGAIN", false).on_press(move |s: &mut State| s.decide(i, true)))
            .child(mini("skip", "SKIP", true).on_press(move |s: &mut State| s.decide(i, false)));
    }
    r
}

// ------------------------------------------------------------------ details

const PANEL_H: f32 = 236.0;

/// The band under a row: closed (no height), or open with the details of `open`.
fn band(s: &State, key: String, open: Option<String>, width: f32) -> El {
    let detail = open.as_deref().and_then(|k| s.detail_of(k).map(|d| (k.to_string(), d)));
    let mut b = el().key(key).w_full().h(if detail.is_some() { PANEL_H + 10.0 } else { 0.0 }).clip_rounded(0.0).motion(Spring::new(OPEN_SECS, 0.0));
    if let Some((k, d)) = detail {
        b = b.child(detail_panel(s, &k, &d, width));
    }
    b
}

fn detail_panel(s: &State, key: &str, d: &Detail, width: f32) -> El {
    let light = paper().alpha(0.55);
    let stagger = |e: El, k: usize| e.transition(Transition::offset(0.0, -10.0).combined(Transition::opacity())).enter_delay(0.04 + k as f32 * 0.045);

    // The cover, or a block in the stripe colour with the first letter.
    let mut cover = el().key("cover").size(PANEL_H - 36.0, PANEL_H - 36.0).border(2.0, paper()).clip_rounded(0.0);
    let pic = if d.path.is_empty() && d.cover.is_empty() { Cover::None } else { s.media.cover(&d.path, &d.cover) };
    cover = match pic {
        Cover::Ready(px) => cover.pixels_fit(&px, SurfaceFit::Fill),
        Cover::Loading => cover.fill(paper().alpha(0.08)),
        Cover::None => {
            let i = d.title.bytes().map(|b| b as usize).sum::<usize>();
            let letter: String = d.title.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
            cover.fill(stripe(i)).center().child(text(letter).font(display(120.0)).color(ink()))
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
    fact("SOURCE", source.into());
    if d.source == "soulseek" {
        fact("FROM", d.peer.to_uppercase());
    }
    fact("QUALITY", if d.bitrate > 0 { format!("{} KBPS", d.bitrate) } else { String::new() });
    fact("SIZE", if d.size > 0 { fmt_size(d.size) } else { String::new() });
    fact("AVG SPEED", if d.speed() > 0.0 { fmt_speed(d.speed() as f32) } else { String::new() });
    fact("TOOK", if d.took() > 0.0 { clock(d.took()) } else { String::new() });
    let mut facts = row().key("facts").gap(26.0).flex_wrap();
    for f in fact_list {
        facts = facts.child(f);
    }

    let sub = [d.artist.clone(), d.album.clone()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    let title_row = row().key("title-row").w_full().gap(14.0).center_y()
        .child(text(d.title.to_uppercase()).font(display(40.0)).color(paper()).truncate().shrink().min_w(0.0))
        .child(row().px(8.0).py(3.0).fill(if d.moved || d.status == "NOT FOUND" { red() } else { yellow() })
            .child(text(d.status.clone()).font(mono(11.0, 600.0)).color(ink())));

    let mut links = row().key("links").gap(22.0);
    if !d.path.is_empty() && !d.path.starts_with("demo://") {
        let p = d.path.clone();
        links = links.child(text("SHOW IN FOLDER ↗").key("reveal").font(mono(12.0, 600.0)).color(paper()).cursor(Cursor::Pointer).role("link")
            .on_press(move |_: &mut State| reveal(&p)));
    }
    if !d.url.is_empty() {
        let u = d.url.clone();
        links = links.child(text("OPEN ON YOUTUBE ↗").key("yt").font(mono(12.0, 600.0)).color(paper()).cursor(Cursor::Pointer).role("link")
            .on_press_tx(move |_: &mut State, tx: &mut Tx| {
                let _ = tx.open_url(&u);
            }));
    }

    let player_el = if d.path.is_empty() {
        text(if d.moved { "THE FILE IS NOT IN THE FOLDER ANY MORE." } else { "THE PLAYER SHOWS UP WHEN THE TRACK IS DOWNLOADED." }).key("no-player").font(mono(12.0, 500.0)).color(light)
    } else {
        player(s, &d.path, d.duration as f64, width)
    };

    row().key(format!("panel-{key}")).w_full().h(PANEL_H).padding(18.0).gap(22.0).fill(ink())
        .transition(Transition::opacity())
        .child(stagger(cover, 0))
        .child(col().key("info").grow(1.0).min_w(0.0).h_full().gap(10.0)
            .child(stagger(col().key("names").w_full().gap(2.0).child(title_row)
                .child(text(sub.to_uppercase()).font(mono(12.0, 500.0)).color(light).truncate()), 1))
            .child(stagger(facts, 2))
            .child(el().grow(1.0))
            .child(stagger(player_el, 3))
            .child(stagger(links, 4)))
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
    let len = track_len(s, path, spotify);
    let pos = match &s.scrub {
        Some((p, f)) if p == path => *f as f64 * len,
        _ if mine => st.pos,
        _ => 0.0,
    };
    let frac = if len > 0.0 { (pos / len).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let playing = mine && st.playing;
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
                let on = (k as f32 + 0.5) / peaks.len() as f32 <= frac;
                bars = bars.child(el().key(format!("b{k}")).grow(1.0).h(4.0 + v * 46.0).radius(1.0).fill(if on { red() } else { paper().alpha(0.4) }));
            }
        }
        Wave::Loading => bars = bars.child(text("READING THE WAVEFORM…").font(mono(11.0, 500.0)).color(paper().alpha(0.5)).cycle(BLINK)),
        Wave::Failed => bars = bars.child(el().grow(1.0).h(2.0).fill(paper().alpha(0.4))),
    }
    let p = path.to_string();
    bars = bars.on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| {
        if s.demo || p.starts_with("demo://") {
            return;
        }
        let f = (d.x / d.w.max(1.0)).clamp(0.0, 1.0);
        match d.phase {
            DragPhase::Down | DragPhase::Move => s.scrub = Some((p.clone(), f)),
            DragPhase::Up => {
                s.scrub = None;
                let len = track_len(s, &p, spotify);
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
        let h = (avail * r.powi(i as i32) / sum).clamp(13.0, 140.0).floor();
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
            row_el = row_el.child(tag("ALREADY HAD"));
        }
        if missing {
            row_el = row_el.child(text("NOT FOUND").font(mono((fs * 0.35).clamp(10.0, 16.0), 600.0)).color(ink()));
        }
        let open = (s.open.as_deref() == Some(key.as_str())).then_some(key);
        stripes = stripes.child(row_el).child(band(s, format!("pband-{i}"), open, size[0]));
    }
    let mb = s.tracks.iter().filter(|t| t.state == TState::Done && !t.youtube).map(|t| t.total).sum::<u64>() as f32 / 1_000_000.0;
    let yt = s.tracks.iter().filter(|t| t.youtube && t.state == TState::Done).count();
    let missing = s.count(TState::Missing);
    let had = s.count(TState::Skipped);
    let mut stats = format!("{} TRACKS", s.count(TState::Done));
    if mb > 0.0 {
        stats += &format!(" · {mb:.0} MB FROM SOULSEEK");
    }
    if yt > 0 {
        stats += &format!(" · {yt} FROM YOUTUBE");
    }
    if had > 0 {
        stats += &format!(" · {had} ALREADY HAD");
    }
    if missing > 0 {
        stats += &format!(" · {missing} MISSING");
    }
    let top = row().key("poster-top").w_full().h(54.0).px(24.0).center_y().justify_between()
        .child(text(format!("DONE. {}", s.name.to_uppercase())).font(mono(15.0, 600.0)).color(ink()).truncate().shrink().min_w(0.0))
        .child(link_text("new-link", "NEW LINK →").on_press(|s: &mut State| s.new_link()));
    let bottom = row().key("poster-bottom").w_full().h(70.0).px(24.0).gap(20.0).center()
        .child(button("open-folder", "OPEN FOLDER →", true).on_press(|s: &mut State| open_folder(if s.out_dir.is_empty() { &s.settings.dir } else { &s.out_dir })))
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
    let n: usize = s.history.sessions.iter().map(|h| h.tracks.iter().filter(|t| t.state == "done").count()).sum();
    let top = row().key("history-top").w_full().h(54.0).px(24.0).center_y().justify_between()
        .child(text(format!("HISTORY · {n} TRACKS")).font(mono(15.0, 600.0)).color(ink()))
        .child(link_text("history-back", "← BACK").on_press(|s: &mut State| {
            s.show_history = false;
            s.open = None;
        }));
    let body = if s.history.sessions.is_empty() {
        col().w_full().grow(1.0).center().child(text("NOTHING DOWNLOADED YET").font(mono(13.0, 600.0)).color(muted()))
    } else {
        col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(history_list(s, "", size[0]))
    };
    col().key("history").w_full().h_full().child(top).child(el().w_full().h(2.0).fill(ink())).child(body)
}

/// "7 NOV 2026".
fn date(unix: f64) -> String {
    const M: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
    let (y, m, d) = crate::model::civil((unix as i64).div_euclid(86_400));
    format!("{d} {} {y}", M[(m as usize).saturating_sub(1).min(11)])
}

/// Every earlier session but `except`: a divider, then each session's tracks.
fn history_list(s: &State, except: &str, width: f32) -> El {
    let mut list = col().key("history-list").w_full().pt(26.0).pb(20.0);
    if !except.is_empty() {
        list = list.child(row().key("divider").w_full().px(24.0).pb(18.0).gap(14.0).center_y()
            .child(el().h(2.0).grow(1.0).fill(ink()))
            .child(text("HISTORY").font(display(30.0)).color(ink()))
            .child(el().h(2.0).grow(1.0).fill(ink())));
    }
    for sess in s.history.sessions.iter().filter(|h| h.id != except) {
        let done = sess.tracks.iter().filter(|t| t.state == "done").count();
        list = list.child(row().key(format!("sess-{}", sess.id)).w_full().h(40.0).px(24.0).gap(12.0).center_y()
            .child(text(sess.name.to_uppercase()).font(mono(14.0, 600.0)).color(ink()).truncate().shrink().min_w(0.0))
            .child(text(format!("{} · {} · {done}/{} TRACKS", sess.kind.to_uppercase(), date(sess.started), sess.tracks.len())).font(mono(11.0, 600.0)).color(muted()))
            .child(el().grow(1.0))
            .child(link_text(&format!("folder-{}", sess.id), "FOLDER ↗").on_press({
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
            let note = if t.state != "done" { "NOT FOUND" } else if gone { "FILE MOVED" } else if t.source == "youtube" { "YT" } else { "" };
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
    let label = |t: &str| text(t.to_string()).font(mono(11.0, 600.0)).color(muted());
    let field = |key: &str, edit: &rune::TextEdit| text_input(edit, mono(15.0, 500.0)).key(key).single_line().w_full().color(ink()).caret(ink(), red().alpha(0.25));
    let boxed = |e: El| row().w_full().h(42.0).px(12.0).center_y().border(2.0, ink()).fill(Color::WHITE).child(e);
    let mut quality = row().gap(0.0);
    for b in ["320", "256", "192", "128"] {
        let on = s.settings.bitrate == b;
        quality = quality.child(row().key(format!("q{b}")).h(40.0).w(70.0).center().border(2.0, ink()).cursor(Cursor::Pointer)
            .fill(if on { ink() } else { Color::TRANSPARENT })
            .on_press(move |s: &mut State| s.settings.bitrate = b.into())
            .child(text(b).font(mono(14.0, 600.0)).color(if on { paper() } else { ink() })));
    }
    let fb = s.settings.fallback;
    let panel = col().key("settings").w(560.0).padding(28.0).gap(12.0).fill(paper()).border(3.0, ink())
        .shadow(18.0, 0.0, ink())
        .on_press(|_: &mut State| {})
        .child(text("SETTINGS").font(display(54.0)).color(ink()))
        .child(label("EXPORT FOLDER"))
        .child(row().w_full().gap(10.0)
            .child(boxed(field("dir", &s.dir_edit).on_edit(|s: &mut State| &mut s.dir_edit)).shrink().grow(1.0).min_w(0.0))
            .child(button("choose-dir", "CHOOSE…", false).on_press_tx(|s: &mut State, tx: &mut Tx| {
                let options = FileDialogOptions { title: "Export folder".into(), directory: Some(s.settings.dir.clone().into()), ..Default::default() };
                tx.open_folder(options, |s: &mut State, result, _| {
                    if let Ok(Some(p)) = result {
                        s.dir_edit.replace_text(p.to_string_lossy().into_owned());
                    }
                });
            })))
        .child(label("MP3 QUALITY (KBPS)"))
        .child(quality)
        .child(label("WHEN SOULSEEK CANNOT FIND A TRACK"))
        .child(row().key("fallback").gap(10.0).center_y().cursor(Cursor::Pointer).on_press(|s: &mut State| s.settings.fallback = !s.settings.fallback)
            .child(row().size(22.0, 22.0).center().border(2.0, ink()).fill(if fb { ink() } else { Color::TRANSPARENT })
                .child(text(if fb { "✓" } else { "" }).font(mono(13.0, 600.0)).color(paper())))
            .child(text("TRY YOUTUBE MUSIC").font(mono(14.0, 600.0)).color(ink())))
        .child(label("SOULSEEK ACCOUNT (EMPTY: USE ~/.config/sockseek/sockseek.conf)"))
        .child(row().w_full().gap(10.0)
            .child(boxed(field("user", &s.user_edit).placeholder("username", muted()).on_edit(|s: &mut State| &mut s.user_edit)).grow(1.0).min_w(0.0))
            .child(boxed(field("pass", &s.pass_edit).secure().placeholder("password (not saved)", muted()).on_edit(|s: &mut State| &mut s.pass_edit)).grow(1.0).min_w(0.0)))
        .child(el().h(6.0))
        .child(row().w_full().justify_end().child(button("save-settings", "DONE", true).on_press(|s: &mut State| s.close_settings())));
    col().key("settings-layer").absolute(0.0, 0.0).w_full().h_full().center()
        .child(el().key("scrim").absolute(0.0, 0.0).w_full().h_full().fill(ink().alpha(0.35)).on_press(|s: &mut State| s.close_settings()))
        .child(panel)
        .transition(Transition::opacity())
}
