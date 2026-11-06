//! The screens: paste a link, watch it work, look at the poster.

use rune::color::hex;
use rune::{Color, Cursor, Cx, El, Family, FileDialogOptions, ScrollSpec, Spring, TextStyle, Transition, Tx, col, el, row, text, text_input};
use rune_motion::Cycle;

use crate::gfx;
use crate::model::{Phase, State, TState, Track};

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
    let field = text_input(&s.link, mono(24.0, 500.0)).key("link").single_line().w_full()
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

fn status(s: &State) -> &'static str {
    if s.phase == Phase::Failed {
        return "FAILED";
    }
    match s.stage.as_str() {
        "spotify" => "READING SPOTIFY",
        "soulseek" if s.downloading() => "DOWNLOADING FROM SOULSEEK",
        "soulseek" => "SEARCHING SOULSEEK",
        "youtube" => "TRYING YOUTUBE MUSIC",
        "convert" => "CONVERTING TO MP3",
        _ => "CONNECTING",
    }
}

fn stat(key: &str, label: &str, value: String) -> El {
    col().key(key).px(18.0).gap(3.0).items_end()
        .child(text(label).font(mono(11.0, 600.0)).color(muted()))
        .child(text(value).font(mono(15.0, 600.0)).color(ink()))
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
    let peer = s.peer().map(|p| format!("FROM {} ⟶", p.to_uppercase())).unwrap_or_else(|| {
        if s.stage == "convert" { format!("CONVERTING {}/{}", s.convert.0, s.convert.1) } else { "WAITING FOR PEERS".into() }
    });
    let bar = col().key("bar-block").w_full().gap(6.0)
        .child(row().w_full().justify_between()
            .child(text(format!("⟵ {}", s.settings.short_dir().to_uppercase())).font(mono(11.0, 600.0)).color(muted()))
            .child(text(peer).key("peer").font(mono(11.0, 600.0)).color(ink())))
        .child(el().key("bar").w_full().h(62.0).radius(31.0).border(2.5, ink()).fill(Color::WHITE)
            .child(el().key("bar-fx").effect(gfx::BAR).effect_data(s.bar()).absolute(0.0, 0.0).w_full().h_full().label(format!("{pct} percent"))))
        .child(row().w_full().justify_between()
            .child(text(format!("{pct}%")).key("pct").font(display(30.0)).color(ink()))
            .child(text(format!("{} DONE · {} LEFT", s.count(TState::Done), n - s.finished())).font(mono(12.0, 600.0)).color(ink())));

    let below: El = if s.phase == Phase::Failed {
        col().key("failed").w_full().gap(16.0).pt(10.0)
            .child(text(if s.error.is_empty() { "Something went wrong.".into() } else { s.error.clone() }).font(mono(13.0, 500.0)).color(red()).wrap())
            .child(row().gap(14.0)
                .child(button("retry", "TRY AGAIN", true).on_press(|s: &mut State| s.start()))
                .child(button("new-link", "NEW LINK", false).on_press(|s: &mut State| s.new_link())))
    } else {
        track_grid(s, size[0] - 56.0)
    };

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

fn track_grid(s: &State, width: f32) -> El {
    let n = s.tracks.len();
    if n == 0 {
        return col().key("tracks").w_full().grow(1.0).center()
            .child(text("READING THE TRACK LIST…").font(mono(13.0, 600.0)).color(muted()).cycle(BLINK));
    }
    let cols = ((width / 250.0).floor() as usize).clamp(1, 6);
    let per = n.div_ceil(cols);
    let cw = width / cols as f32;
    let mut grid = row().key("track-cols").w_full().items_start();
    for ci in 0..cols {
        let mut column = col().key(format!("col-{ci}")).w(cw).pr(10.0).gap(2.0);
        for i in ci * per..((ci + 1) * per).min(n) {
            column = column.child(cell(i, &s.tracks[i]));
        }
        grid = grid.child(column);
    }
    col().key("tracks").w_full().grow(1.0).min_h(0.0).pb(10.0).scroll(ScrollSpec { stick_bottom: false }).child(grid)
}

fn cell(i: usize, t: &Track) -> El {
    let title = t.title.to_uppercase();
    let (bg, fg, chip): (Color, Color, String) = match t.state {
        TState::Pending => (Color::TRANSPARENT, muted(), String::new()),
        TState::Searching => (Color::TRANSPARENT, ink(), "…".into()),
        TState::Downloading => {
            let p = if t.total > 0 { t.bytes as f32 / t.total as f32 } else { 0.0 };
            (ink(), paper(), format!("{:.0}%", p * 100.0))
        }
        TState::Done => (stripe(i), ink(), if t.youtube { "YT".into() } else { "OK".into() }),
        TState::Failed => (Color::TRANSPARENT, red(), "NOT FOUND".into()),
        TState::YtSearch | TState::YtDownload => (yellow(), ink(), "→ YT".into()),
        TState::Missing => (Color::TRANSPARENT, muted(), "MISSING".into()),
    };
    let mut label = text(title.clone()).font(mono(12.5, 600.0)).color(fg).truncate().grow(1.0).min_w(0.0);
    if t.state == TState::Missing {
        label = label.strike(0, title.len(), Some(muted()));
    }
    let mut chip_el = text(chip).font(mono(11.0, 600.0)).color(fg).ml(8.0);
    if matches!(t.state, TState::Searching | TState::YtSearch | TState::YtDownload) {
        chip_el = chip_el.cycle(BLINK);
    }
    let mut r = row().key(format!("t{i}")).w_full().h(24.0).pl(6.0).pr(6.0).center_y().fill(bg)
        .motion(Spring::new(0.35, 0.1))
        .transition(Transition::offset(0.0, 12.0).combined(Transition::opacity()))
        .enter_delay((i as f32 * 0.018).min(1.2));
    // A red fill that grows behind the title as bytes arrive. It is always there (empty when
    // not downloading), so the row keeps the same children in every state.
    let p = if t.state == TState::Downloading && t.total > 0 { t.bytes as f32 / t.total as f32 } else { 0.0 };
    r = r.child(el().key("fill").absolute(0.0, 0.0).h_full().w_frac(p).fill(if p > 0.0 { red() } else { Color::TRANSPARENT }).motion(Spring::new(0.5, 0.0)));
    r.child(text(format!("{:>2}.", i + 1)).key("num").font(mono(12.0, 500.0)).color(fg).w(36.0))
        .child(label.key("title"))
        .child(chip_el.key("chip"))
}

// ------------------------------------------------------------------ 3. poster

fn poster(s: &State, size: [f32; 2]) -> El {
    let n = s.tracks.len().max(1);
    let avail = size[1] - 54.0 - 70.0;
    // The first stripes are the biggest; each one is a little smaller than the one before,
    // and together they fill the window (or scroll, for very long lists).
    let r = (1.0f32 / 3.0).powf(1.0 / (n as f32 - 1.0).max(1.0));
    let sum: f32 = (0..n).map(|i| r.powi(i as i32)).sum();
    let mut stripes = col().key("stripes").w_full();
    for (i, t) in s.tracks.iter().enumerate() {
        let h = (avail * r.powi(i as i32) / sum).clamp(13.0, 140.0).floor();
        let fs = (h * 0.9).floor();
        let title = t.title.to_uppercase();
        let missing = t.state != TState::Done;
        let mut label = text(title.clone()).font(display(fs)).color(ink()).truncate().shrink().min_w(0.0);
        if missing {
            label = label.strike(0, title.len(), Some(ink()));
        }
        let mut r = row().key(format!("p{i}")).w_full().h(h).px(28.0).gap(14.0).center_y().clip_rounded(0.0)
            .fill(if missing { c("#bdbdbb") } else { stripe(i) })
            .transition(Transition::offset(-60.0, 0.0).combined(Transition::opacity()))
            .enter_delay((i as f32 * 0.035).min(1.6))
            .child(label)
            .child(el().grow(1.0));
        if t.youtube && !missing {
            r = r.child(row().px(6.0).py(2.0).fill(ink()).child(text("YT").font(mono((fs * 0.4).clamp(10.0, 18.0), 600.0)).color(paper())));
        }
        if missing {
            r = r.child(text("NOT FOUND").font(mono((fs * 0.35).clamp(10.0, 16.0), 600.0)).color(ink()));
        }
        stripes = stripes.child(r);
    }
    let mb = s.tracks.iter().filter(|t| t.state == TState::Done && !t.youtube).map(|t| t.total).sum::<u64>() as f32 / 1_000_000.0;
    let yt = s.tracks.iter().filter(|t| t.youtube && t.state == TState::Done).count();
    let missing = s.tracks.iter().filter(|t| t.state != TState::Done).count();
    let mut stats = format!("{} TRACKS", s.count(TState::Done));
    if mb > 0.0 {
        stats += &format!(" · {mb:.0} MB FROM SOULSEEK");
    }
    if yt > 0 {
        stats += &format!(" · {yt} FROM YOUTUBE");
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
    col().key("poster").w_full().h_full()
        .child(top)
        .child(col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(stripes))
        .child(bottom)
}

pub fn open_folder(path: &str) {
    let cmd = if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = std::process::Command::new(cmd).arg(path).spawn();
}

// ------------------------------------------------------------------ settings

fn settings_panel(s: &State) -> El {
    let label = |t: &str| text(t.to_string()).font(mono(11.0, 600.0)).color(muted());
    let field = |key: &str, edit: &rune::TextEdit| text_input(edit, mono(15.0, 500.0)).key(key).single_line().w_full().color(ink());
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
