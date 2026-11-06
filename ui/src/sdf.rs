//! Signed distance fields of headline words. The headline shader reads two of them and blends
//! the distances, so one word can flow into the next like a liquid.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};


pub const ANTON: &[u8] = include_bytes!("../assets/Anton-Regular.ttf");

/// Distance (in texels) that the field encodes on each side of the edge.
pub const SPREAD: f32 = 32.0;
/// Font size of the rasterised words, in texels.
const PX: f32 = 360.0;
/// Line advance as a multiple of the cap height (the words are all caps).
const LEADING: f32 = 1.16;
const PAD: usize = SPREAD as usize + 4;

/// One word (or block of lines) as a distance field.
pub struct Field {
    pub w: u32,
    pub h: u32,
    /// R8: 0.5 is the glyph edge, 1.0 is `SPREAD` texels inside, 0.0 is `SPREAD` outside.
    pub data: Vec<u8>,
}

/// Rasterises `lines` (left aligned) in Anton and builds the distance field.
pub fn field(lines: &[&str]) -> Field {
    let font = FontRef::try_from_slice(ANTON).expect("Anton font");
    let sf = font.as_scaled(PxScale::from(PX));
    // Cap height in texels: the words are upper case, so trim the empty space above the caps.
    let cap = font
        .outline_glyph(sf.scaled_glyph('H'))
        .map(|o| o.px_bounds().height())
        .unwrap_or(sf.ascent() * 0.8);
    let line_h = cap * LEADING;
    let tracking = PX * 0.01;

    let widths: Vec<f32> = lines
        .iter()
        .map(|l| {
            let mut x = 0.0;
            let mut prev = None;
            for c in l.chars() {
                let id = sf.glyph_id(c);
                if let Some(p) = prev {
                    x += sf.kern(p, id);
                }
                x += sf.h_advance(id) + tracking;
                prev = Some(id);
            }
            x - tracking
        })
        .collect();
    let text_w = widths.iter().cloned().fold(0.0, f32::max).ceil() as usize;
    let text_h = (cap + line_h * (lines.len() as f32 - 1.0)).ceil() as usize;
    let w = text_w + PAD * 2;
    let h = text_h + PAD * 2;

    let mut cov = vec![0f32; w * h];
    for (li, l) in lines.iter().enumerate() {
        let baseline = PAD as f32 + cap + line_h * li as f32;
        let mut x = PAD as f32;
        let mut prev = None;
        for c in l.chars() {
            let id = sf.glyph_id(c);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(PX, point(x, baseline));
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, v| {
                    let px = b.min.x as i32 + gx as i32;
                    let py = b.min.y as i32 + gy as i32;
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        let i = py as usize * w + px as usize;
                        cov[i] = (cov[i] + v).min(1.0);
                    }
                });
            }
            x += sf.h_advance(id) + tracking;
            prev = Some(id);
        }
    }

    let data = distance(&cov, w, h);
    Field { w: w as u32, h: h as u32, data }
}

/// Signed distance from coverage: exact Euclidean transforms of the inside and the outside,
/// with the antialiased edge folded in as a sub-texel offset.
fn distance(cov: &[f32], w: usize, h: usize) -> Vec<u8> {
    const INF: f64 = 1e20;
    let mut outside = vec![0f64; w * h];
    let mut inside = vec![0f64; w * h];
    for i in 0..w * h {
        // The rasteriser leaves tiny non-zero values inside a glyph's box; treat them as paper.
        let c = cov[i] as f64;
        let c = if c < 0.02 { 0.0 } else if c > 0.98 { 1.0 } else { c };
        outside[i] = if c >= 0.5 { 0.0 } else if c > 0.0 { (0.5 - c).powi(2) } else { INF };
        inside[i] = if c <= 0.5 { 0.0 } else if c < 1.0 { (c - 0.5).powi(2) } else { INF };
    }
    edt(&mut outside, w, h);
    edt(&mut inside, w, h);
    let mut out = vec![0u8; w * h];
    for i in 0..w * h {
        let d = outside[i].sqrt() - inside[i].sqrt(); // > 0 outside the glyph
        let v = 0.5 - d / (2.0 * SPREAD as f64);
        out[i] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    out
}

/// Felzenszwalb and Huttenlocher's squared distance transform, in place, rows then columns.
fn edt(grid: &mut [f64], w: usize, h: usize) {
    let n = w.max(h);
    let mut f = vec![0f64; n];
    let mut d = vec![0f64; n];
    let mut v = vec![0usize; n];
    let mut z = vec![0f64; n + 1];
    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        edt1(&f[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&grid[y * w..y * w + w]);
        edt1(&f[..w], &mut d[..w], &mut v, &mut z);
        grid[y * w..y * w + w].copy_from_slice(&d[..w]);
    }
}

fn edt1(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let mut s;
        loop {
            let r = v[k];
            s = ((f[q] + (q * q) as f64) - (f[r] + (r * r) as f64)) / (2.0 * q as f64 - 2.0 * r as f64);
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                break;
            }
        }
        if s <= z[k] {
            // k == 0 and the new parabola wins everywhere.
            v[0] = q;
            z[0] = f64::NEG_INFINITY;
            z[1] = f64::INFINITY;
            continue;
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let r = v[k];
        let dq = q as f64 - r as f64;
        d[q] = dq * dq + f[r];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_has_ink_inside_and_paper_outside() {
        let f = field(&["SEARCHING"]);
        assert!(f.w > f.h * 3, "one long line: {}x{}", f.w, f.h);
        // Corners are paper, far from any glyph.
        assert_eq!(f.data[0], 0);
        // Some texels are deep inside a stroke.
        assert!(f.data.iter().any(|&v| v > 200));
    }

    #[test]
    fn the_transform_matches_brute_force_on_a_small_grid() {
        let (w, h) = (9, 7);
        let mut g = vec![1e20f64; w * h];
        g[3 * w + 4] = 0.0;
        edt(&mut g, w, h);
        for y in 0..h {
            for x in 0..w {
                let want = (x as f64 - 4.0).powi(2) + (y as f64 - 3.0).powi(2);
                assert!((g[y * w + x] - want).abs() < 1e-3);
            }
        }
    }
}
