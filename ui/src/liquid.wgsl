// Spotseek's two custom effects, drawn at Rune effect slots.
//
// Mode 0, the headline: two words as signed distance fields. Their distances are blended with a
// noisy, per-pixel progress, so one word melts and flows into the next. On top: a water wobble
// (the word moves like a liquid, but keeps its flat ink) and a glitch (sliced rows and red/blue
// fringes).
//
// Mode 1, the progress bar: a pool of liquid metal that fills the tube, fed by drops that travel
// from the peer (the blob at the right end) into the pool. Drops and pool join with a smooth
// minimum, so they merge like mercury.

struct U {
    rect: vec4f,    // slot rect in pass pixels: x, y, w, h
    view: vec4f,    // pass width, pass height, pixels per point, srgb (1 or 0)
    a_fit: vec4f,   // headline: word A's box in slot pixels
    b_fit: vec4f,   // headline: word B's box in slot pixels
    p0: vec4f,      // headline: t, morph, wobble, glitch | bar: t, fill, drop phase, active
    p1: vec4f,      // headline: A px/texel, B px/texel, mode, 0 | bar: splash, flow, mode, done
    ink: vec4f,     // ink colour, sRGB
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var tex_a: texture_2d<f32>;
@group(0) @binding(2) var tex_b: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

const SPREAD: f32 = 32.0;
const PI: f32 = 3.14159265;

struct VOut {
    @builtin(position) pos: vec4f,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    var corners = array<vec2f, 6>(vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0), vec2f(0.0, 1.0));
    let c = corners[vi];
    let px = u.rect.xy + c * u.rect.zw;
    let ndc = vec2f(px.x / u.view.x * 2.0 - 1.0, 1.0 - px.y / u.view.y * 2.0);
    var o: VOut;
    o.pos = vec4f(ndc, 0.0, 1.0);
    return o;
}

// ------------------------------------------------------------------ noise

fn hash11(n: f32) -> f32 {
    return fract(sin(n * 127.1) * 43758.5453);
}

fn hash21(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2f(1.0, 0.0));
    let c = hash21(i + vec2f(0.0, 1.0));
    let d = hash21(i + vec2f(1.0, 1.0));
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y);
}

fn fbm(p: vec2f) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 4; i++) {
        v += a * noise(q);
        q = q * 2.03 + vec2f(1.7, 9.2);
        a *= 0.5;
    }
    return v;
}

fn to_linear(c: vec3f) -> vec3f {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3f(2.4));
    return select(hi, lo, c <= vec3f(0.04045));
}

// Colours are authored in sRGB. A linear-blended frame wants linear values.
fn out_color(c: vec3f, a: f32) -> vec4f {
    var rgb = c;
    if (u.view.w < 0.5) {
        rgb = to_linear(c);
    }
    return vec4f(rgb * a, a);
}

// ------------------------------------------------------------------ headline

// Distance to word A or B at slot pixel `p`, in pixels (> 0 outside).
fn word(which: i32, p: vec2f) -> f32 {
    var fit = u.a_fit;
    var k = u.p1.x;
    if (which == 1) {
        fit = u.b_fit;
        k = u.p1.y;
    }
    let uv = (p - fit.xy) / fit.zw;
    if (any(uv < vec2f(0.0)) || any(uv > vec2f(1.0))) {
        return SPREAD * k;
    }
    var v = 0.0;
    if (which == 0) {
        v = textureSampleLevel(tex_a, samp, uv, 0.0).r;
    } else {
        v = textureSampleLevel(tex_b, samp, uv, 0.0).r;
    }
    return (0.5 - v) * 2.0 * SPREAD * k;
}

// The liquid offset at point `q` (points), in points.
fn flow(q: vec2f, t: f32, wob: f32) -> vec2f {
    let a = vec2f(
        sin(q.y * 0.045 + t * 2.1) * 5.0 + (fbm(q * 0.011 + vec2f(t * 0.22, 0.0)) - 0.5) * 26.0,
        sin(q.x * 0.028 + t * 1.6) * 4.0 + (fbm(q * 0.011 + vec2f(3.1, t * 0.27)) - 0.5) * 18.0,
    );
    return a * wob;
}

fn glitch_shift(q: vec2f, t: f32, g: f32) -> f32 {
    if (g <= 0.001) {
        return 0.0;
    }
    let tick = floor(t * 13.0);
    let band = floor(q.y / 7.0);
    let r = hash21(vec2f(band, tick));
    var dx = 0.0;
    if (r > 1.0 - 0.28 * g) {
        dx = (hash21(vec2f(band * 3.7, tick + 5.0)) - 0.5) * 70.0 * g;
    }
    // The wavy smear of a bad tape.
    let wave = sin(q.y * 0.42 + t * 17.0) * sin(q.y * 0.07 - t * 3.0);
    dx += wave * 3.5 * g;
    return dx;
}

fn headline_d(q: vec2f, s: f32, t: f32, morph: f32, wob: f32, g: f32, shift: f32) -> f32 {
    // Per-pixel progress of the morph: noisy, so the word melts in blobs, not as a fade.
    let n = fbm(q * 0.0075 + vec2f(11.0, 4.0));
    let mm = smoothstep(0.0, 1.0, clamp(morph * 1.5 - n * 0.5, 0.0, 1.0));
    let melt = sin(PI * mm);
    // While it melts, the ink drips: sample from above, so ink runs down.
    let drip = vec2f(0.0, -melt * (fbm(vec2f(q.x * 0.035, 2.0)) * 46.0 + 6.0));
    let jitter = vec2f(shift + glitch_shift(q, t, g), 0.0);
    let base = q + flow(q, t, max(wob, melt * 0.8)) + drip + jitter;
    var d = 0.0;
    if (mm <= 0.0) {
        d = word(0, base * s);
    } else if (mm >= 1.0) {
        d = word(1, base * s);
    } else {
        d = mix(word(0, base * s), word(1, base * s), mm);
    }
    // Swell while melting, so the blobs of both words touch and merge like a liquid.
    d -= melt * 7.0 * s;
    // A slow breathing of the stroke while the liquid moves.
    d -= wob * sin(q.x * 0.05 + q.y * 0.03 + t * 3.0) * 1.2 * s;
    return d;
}

fn fs_headline(p: vec2f) -> vec4f {
    let s = u.view.z;
    let t = u.p0.x;
    let morph = u.p0.y;
    let wob = u.p0.z;
    let g = u.p0.w;
    let q = p / s;
    let d0 = headline_d(q, s, t, morph, wob, g, 0.0);
    let c0 = clamp(0.5 - d0, 0.0, 1.0);
    let ink = u.ink.rgb;
    if (g <= 0.001) {
        return out_color(ink, c0);
    }
    // Red and blue copies pulled apart: the glitch's fringes.
    let off = 3.0 + 7.0 * g * (0.5 + 0.5 * sin(t * 9.0));
    let c1 = clamp(0.5 - headline_d(q, s, t, morph, wob, g, off), 0.0, 1.0) * g;
    let c2 = clamp(0.5 - headline_d(q, s, t, morph, wob, g, -off), 0.0, 1.0) * g;
    let red = vec3f(0.91, 0.2, 0.12);
    let blue = vec3f(0.1, 0.25, 0.82);
    var rgb = red * c1;
    var a = c1;
    rgb = blue * c2 + rgb * (1.0 - c2);
    a = c2 + a * (1.0 - c2);
    rgb = ink * c0 + rgb * (1.0 - c0);
    a = c0 + a * (1.0 - c0);
    var lin = rgb;
    if (u.view.w < 0.5) {
        // rgb is premultiplied here; convert the straight colour.
        let st = rgb / max(a, 1e-4);
        lin = to_linear(st) * a;
    }
    return vec4f(lin, a);
}

// ------------------------------------------------------------------ progress bar

fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return mix(b, a, h) - k * h * (1.0 - h);
}

fn sd_round_box(p: vec2f, b: vec2f, r: f32) -> f32 {
    let q = abs(p) - b + vec2f(r);
    return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// Liquid metal field in points (> 0 outside).
fn bar_d(q: vec2f) -> f32 {
    let s = u.view.z;
    let size = u.rect.zw / s;
    let t = u.p0.x;
    let fill = u.p0.y;
    let phase = u.p0.z;
    let on = u.p0.w;
    let splash = u.p1.x;
    let pad = 5.0;
    let r = (size.y - pad * 2.0) * 0.5;
    let cy = size.y * 0.5;
    let src = vec2f(size.x - pad - r, cy);
    let front = mix(r * 1.2, src.x - r * 1.4, clamp(fill, 0.0, 1.0));

    // The pool, with a sloshing front edge.
    let slosh = (sin(q.y * 0.23 + t * 6.0) * 2.2 + sin(q.y * 0.61 - t * 9.0) * 1.1) * (0.5 + on * 0.5 + splash * 4.0);
    let left = pad;
    let cx = (left + front) * 0.5;
    let hw = max((front - left) * 0.5, 0.5);
    var d = sd_round_box(vec2f(q.x + slosh * smoothstep(left, front, q.x) - cx, q.y - cy), vec2f(hw, r * 0.92), r * 0.92);

    if (on > 0.01) {
        // The peer: a blob that breathes while it sends.
        let pulse = 0.72 + 0.08 * sin(t * 5.3) + 0.05 * sin(t * 11.0);
        let blob = length(q - src) - r * pulse * on;
        d = smin(d, blob, r * 0.8);
        // Drops on their way from the peer to the pool.
        for (var k = 0; k < 9; k++) {
            let kf = f32(k);
            let ph = fract(phase + kf / 9.0 + hash11(kf) * 0.05);
            let e = ph * ph;
            let x = mix(src.x, front, e);
            let y = cy + sin(ph * PI * 3.0 + kf * 1.7) * r * 0.32;
            let rr = r * (0.26 + 0.2 * hash11(kf + 3.0)) * (1.0 - 0.35 * ph) * on;
            d = smin(d, length(q - vec2f(x, y)) - rr, r * 0.55);
        }
    }
    return d;
}

fn chrome(n: vec3f, q: vec2f, t: f32) -> vec3f {
    // A studio environment: bright sky, a dark horizon band, a grey floor.
    let v = n.y * 0.85 + n.x * 0.3;
    var c = mix(vec3f(0.07, 0.07, 0.09), vec3f(0.97, 0.97, 1.0), smoothstep(-0.22, 0.12, -v));
    c = mix(c, vec3f(0.42, 0.44, 0.5), smoothstep(0.35, 0.75, v));
    c = mix(c, vec3f(0.12, 0.12, 0.15), smoothstep(0.05, 0.0, abs(v + 0.42)) * 0.55);
    // A thin film of colour in the bright parts, as on hot metal.
    let film = 0.5 + 0.5 * sin(n.x * 5.0 + n.y * 3.0 + t * 0.6);
    let tint = mix(vec3f(1.0, 0.86, 0.9), vec3f(0.84, 0.92, 1.0), film);
    c *= mix(vec3f(1.0), tint, 0.35);
    let l = normalize(vec3f(-0.45, -0.7, 1.0));
    let spec = pow(max(dot(n, l), 0.0), 70.0);
    return c + vec3f(spec * 0.9);
}

fn fs_bar(p: vec2f) -> vec4f {
    let s = u.view.z;
    let t = u.p0.x;
    let q = p / s;
    let d = bar_d(q);
    let a = clamp(0.5 - d * s, 0.0, 1.0);
    if (a <= 0.0) {
        return vec4f(0.0);
    }
    let size = u.rect.zw / s;
    let r = (size.y - 10.0) * 0.5;
    let e = 0.6;
    let gx = bar_d(q + vec2f(e, 0.0)) - bar_d(q - vec2f(e, 0.0));
    let gy = bar_d(q + vec2f(0.0, e)) - bar_d(q - vec2f(0.0, e));
    let depth = clamp(-d / (r * 0.85), 0.0, 1.0);
    let rim = 1.0 - depth;
    let h = sqrt(max(1.0 - rim * rim, 0.0));
    var g2 = vec2f(gx, gy);
    if (length(g2) > 1e-5) {
        g2 = normalize(g2);
    }
    // Ripples on the surface while the metal flows.
    let flowk = u.p1.y;
    let rip = vec2f(sin(q.x * 0.35 - t * 5.0 + sin(q.y * 0.4 + t)), cos(q.y * 0.5 + q.x * 0.08 + t * 3.0)) * 0.18 * flowk;
    var n = normalize(vec3f(g2 * rim * 1.9 + rip, max(h, 0.08)));
    var c = chrome(n, q, t);
    // An ink outline, for the poster look.
    let line = smoothstep(1.6, 0.6, -d * s);
    c = mix(c, u.ink.rgb, line * 0.9);
    return out_color(c, a);
}

@fragment
fn fs(in: VOut) -> @location(0) vec4f {
    let p = in.pos.xy - u.rect.xy;
    if (u.p1.z > 0.5) {
        return fs_bar(p);
    }
    return fs_headline(p);
}
