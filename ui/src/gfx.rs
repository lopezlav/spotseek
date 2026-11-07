//! The compositor. Rune draws the whole UI; at two effect slots this draws Spotseek's own shader
//! (`liquid.wgsl`): the liquid headline and the liquid-metal progress bar.
//!
//! The view tells the compositor what it wants through `effect_data`:
//! - headline: `[word, wobble, glitch, 0]`, where `word` is an index into [`WORDS`];
//! - bar: `[fill 0..1, speed 0..1, peers sending 0..4, tracks done]`.
//! The compositor owns the motion: it morphs to a new word over [`MORPH`] seconds, eases the
//! wobble and glitch, springs the fill and moves the drops.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rune::compose::{Compositor, DeviceKey, Frame};
use rune::render::{EffectPhase, EffectSite, KIND_EFFECT};

use crate::sdf;
use crate::State;

pub const HEADLINE: u32 = 1;
pub const BAR: u32 = 2;

/// The headline words, as lines. The index is the `word` in the headline's effect data.
pub const WORDS_PER_LANG: usize = 7;
/// The English words, then the same words in Spanish.
pub const WORDS: [&[&str]; 14] = [
    &["PASTE A", "SPOTIFY LINK."],
    &["SEARCHING"],
    &["DOWNLOADING"],
    &["CONVERTING"],
    &["DONE."],
    &["FAILED."],
    &["ALREADY", "DOWNLOADED."],
    &["PEGÁ UN LINK", "DE SPOTIFY."],
    &["BUSCANDO"],
    &["DESCARGANDO"],
    &["CONVIRTIENDO"],
    &["LISTO."],
    &["FALLÓ."],
    &["YA LO", "TENÉS."],
];
pub const W_PASTE: f32 = 0.0;
pub const W_SEARCHING: f32 = 1.0;
pub const W_DOWNLOADING: f32 = 2.0;
pub const W_CONVERTING: f32 = 3.0;
pub const W_DONE: f32 = 4.0;
pub const W_FAILED: f32 = 5.0;
pub const W_HAVE: f32 = 6.0;

/// Seconds a word takes to melt into the next.
pub const MORPH: f64 = 1.5;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    rect: [f32; 4],
    view: [f32; 4],
    a_fit: [f32; 4],
    b_fit: [f32; 4],
    p0: [f32; 4],
    p1: [f32; 4],
    ink: [f32; 4],
}

struct WordTex {
    view: wgpu::TextureView,
    w: f32,
    h: f32,
}

struct Slot {
    buf: wgpu::Buffer,
    bind: Option<(u32, u32, wgpu::BindGroup)>,
}

#[derive(Default)]
struct Head {
    cur: u32,
    prev: u32,
    t0: f64,
    wob: f32,
    glitch: f32,
    seen: bool,
}

#[derive(Default)]
struct Bar {
    fill: f32,
    vel: f32,
    phase: f32,
    active: f32,
    /// Peers sending, eased, so a new blob grows in.
    peers: f32,
    done: f32,
    splash_t: f64,
    seen: bool,
}

struct Gpu {
    key: DeviceKey,
    layout: wgpu::BindGroupLayout,
    pipe_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    pipes: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    words: HashMap<u32, WordTex>,
    slots: HashMap<u32, Slot>,
    blank: wgpu::TextureView,
}

pub struct Gfx {
    gpu: Option<Gpu>,
    /// Distance fields made on a worker thread at start, by word index.
    fields: Arc<Mutex<HashMap<u32, sdf::Field>>>,
    head: Head,
    bar: Bar,
    last_t: Option<f64>,
    moving: bool,
}

impl Default for Gfx {
    fn default() -> Self {
        let fields: Arc<Mutex<HashMap<u32, sdf::Field>>> = Arc::default();
        let worker = fields.clone();
        std::thread::spawn(move || {
            for (i, lines) in WORDS.iter().enumerate() {
                let f = sdf::field(lines);
                worker.lock().unwrap().insert(i as u32, f);
            }
        });
        Gfx { gpu: None, fields, head: Head::default(), bar: Bar::default(), last_t: None, moving: false }
    }
}

impl Gpu {
    fn new(device: &wgpu::Device, key: DeviceKey) -> Gpu {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("spotseek-liquid"),
            source: wgpu::ShaderSource::Wgsl(include_str!("liquid.wgsl").into()),
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spotseek-liquid"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                tex_entry(1),
                tex_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("spotseek-liquid"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("spotseek-sdf"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blank = Self::upload(device, None, 1, 1, &[0]);
        Gpu { key, layout, pipe_layout, module, sampler, pipes: HashMap::new(), words: HashMap::new(), slots: HashMap::new(), blank }
    }

    fn upload(device: &wgpu::Device, queue: Option<&wgpu::Queue>, w: u32, h: u32, data: &[u8]) -> wgpu::TextureView {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("spotseek-sdf"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        if let Some(queue) = queue {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        }
        tex.create_view(&Default::default())
    }

    fn pipe(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let (module, layout) = (&self.module, &self.pipe_layout);
        self.pipes.entry(format).or_insert_with(|| {
            let blend = wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("spotseek-liquid"),
                layout: Some(layout),
                vertex: wgpu::VertexState { module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState { color: blend, alpha: blend }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }
}

impl Gfx {
    /// Makes sure word `i` is on the GPU, making its field now if the worker hasn't yet.
    fn ensure_word(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, i: u32) {
        let gpu = self.gpu.as_mut().unwrap();
        if gpu.words.contains_key(&i) {
            return;
        }
        let field = self.fields.lock().unwrap().remove(&i).unwrap_or_else(|| sdf::field(WORDS[i as usize]));
        let view = Gpu::upload(device, Some(queue), field.w, field.h, &field.data);
        gpu.words.insert(i, WordTex { view, w: field.w as f32, h: field.h as f32 });
    }

    fn step(&mut self, t: f64, dt: f32, head: Option<[f32; 4]>, bar: Option<[f32; 4]>) {
        let ease = |k: f32| 1.0 - (-dt * k).exp();
        if let Some(d) = head {
            let word = (d[0].max(0.0) as u32).min(WORDS.len() as u32 - 1);
            let h = &mut self.head;
            if !h.seen {
                h.seen = true;
                h.cur = word;
                h.prev = word;
                h.t0 = t - MORPH;
                h.wob = d[1];
                h.glitch = d[2];
            } else if word != h.cur {
                h.prev = h.cur;
                h.cur = word;
                h.t0 = t;
            }
            h.wob += (d[1] - h.wob) * ease(2.5);
            h.glitch += (d[2] - h.glitch) * ease(4.0);
            if h.wob < 0.002 && d[1] == 0.0 {
                h.wob = 0.0;
            }
            if h.glitch < 0.002 && d[2] == 0.0 {
                h.glitch = 0.0;
            }
        }
        if let Some(d) = bar {
            let b = &mut self.bar;
            if !b.seen {
                b.seen = true;
                b.fill = d[0];
                b.active = d[2].min(1.0);
                b.peers = d[2];
                b.done = d[3];
            }
            // A slightly bouncy spring toward the fill the view asks for.
            let (w, z) = (7.0f32, 0.55f32);
            let acc = w * w * (d[0] - b.fill) - 2.0 * z * w * b.vel;
            b.vel += acc * dt;
            b.fill += b.vel * dt;
            b.active += (d[2].min(1.0) - b.active) * ease(3.0);
            b.peers += (d[2] - b.peers) * ease(4.0);
            b.phase = (b.phase + dt * (0.18 + d[1].clamp(0.0, 1.0) * 0.9) * b.active).fract();
            if d[3] > b.done {
                b.splash_t = t;
            }
            b.done = d[3];
        }
    }

    fn morph(&self, t: f64) -> f32 {
        // Ease out: the melt starts at once and settles slowly.
        let x = (((t - self.head.t0) / MORPH) as f32).clamp(0.0, 1.0);
        1.0 - (1.0 - x) * (1.0 - x)
    }
}

impl Compositor<State> for Gfx {
    fn animating(&self) -> bool {
        self.moving
    }

    fn render(&mut self, f: &mut Frame<'_, State>) {
        let (device, queue, size) = (f.device, f.queue, f.size);
        let key = f.device_key();
        if self.gpu.as_ref().is_none_or(|g| g.key != key) {
            self.gpu = Some(Gpu::new(device, key));
        }
        let t = f.rt.t;
        let dt = self.last_t.map_or(0.0, |l| (t - l).clamp(0.0, 0.1)) as f32;
        self.last_t = Some(t);

        // What the view asks for this frame.
        let (mut head, mut bar) = (None, None);
        for (n, i) in f.rt.instances.iter().enumerate() {
            if i.params[1] != KIND_EFFECT {
                continue;
            }
            match f.rt.marks.get(n).copied().unwrap_or(0) {
                HEADLINE => head = Some(i.uv),
                BAR => bar = Some(i.uv),
                _ => {}
            }
        }
        self.step(t, dt, head, bar);
        if head.is_some() {
            self.ensure_word(device, queue, self.head.cur);
            self.ensure_word(device, queue, self.head.prev);
        }
        let morph = self.morph(t);
        let splash = (-(t - self.bar.splash_t) as f32 * 3.0).exp();
        self.moving = (head.is_some() && (self.head.wob > 0.0 || self.head.glitch > 0.0 || morph < 1.0))
            || (bar.is_some() && (self.bar.active > 0.01 || self.bar.vel.abs() > 0.001 || splash > 0.01));

        // Per-slot buffers and bind groups.
        let gpu = self.gpu.as_mut().unwrap();
        for mark in [HEADLINE, BAR] {
            let present = if mark == HEADLINE { head.is_some() } else { bar.is_some() };
            if !present {
                continue;
            }
            let slot = gpu.slots.entry(mark).or_insert_with(|| Slot {
                buf: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("spotseek-liquid"),
                    size: std::mem::size_of::<Uniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                bind: None,
            });
            let (a, b) = if mark == HEADLINE { (self.head.prev, self.head.cur) } else { (u32::MAX, u32::MAX) };
            if slot.bind.as_ref().is_none_or(|(x, y, _)| (*x, *y) != (a, b)) {
                let va = gpu.words.get(&a).map_or(&gpu.blank, |w| &w.view);
                let vb = gpu.words.get(&b).map_or(&gpu.blank, |w| &w.view);
                let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("spotseek-liquid"),
                    layout: &gpu.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: slot.buf.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(va) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(vb) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&gpu.sampler) },
                    ],
                });
                slot.bind = Some((a, b, bg));
            }
        }
        for fmt in [f.format, wgpu::TextureFormat::Rgba16Float] {
            gpu.pipe(device, fmt);
        }

        let (hd, bd) = (&self.head, &self.bar);
        let word_dims = |i: u32| gpu.words.get(&i).map_or((1.0, 1.0), |w| (w.w, w.h));
        let (aw, ah) = word_dims(hd.prev);
        let (bw, bh) = word_dims(hd.cur);
        let glitch = hd.glitch.max(if morph < 1.0 { (1.0 - morph) * 0.9 * (morph * 6.0).min(1.0) } else { 0.0 });
        let clear = f.rt.clear_color();
        f.ui.prepare(device, queue, &mut f.rt.text.atlas, &f.rt.instances, size);
        let pipes = &gpu.pipes;
        let slots = &gpu.slots;
        f.ui.draw_with_backdrop(f.encoder, f.target, clear, &mut |phase, s: &EffectSite<'_>| {
            let EffectPhase::Draw(pass) = phase else { return };
            let (Some(slot), Some(pipe)) = (slots.get(&s.mark), pipes.get(&s.format)) else { return };
            let Some((_, _, bind)) = slot.bind.as_ref() else { return };
            let r = s.instance.rect;
            let rect = [r[0] - s.origin[0], r[1] - s.origin[1], r[2], r[3]];
            let view = [s.size[0] as f32, s.size[1] as f32, s.scale, if s.srgb_blend { 1.0 } else { 0.0 }];
            let ink = [0.04, 0.04, 0.04, 1.0];
            let u = if s.mark == HEADLINE {
                // Fit each word into the slot: as large as it goes, centred.
                let fit = |w: f32, h: f32| {
                    let k = (rect[2] / w).min(rect[3] / h);
                    let (dw, dh) = (w * k, h * k);
                    ([(rect[2] - dw) * 0.5, (rect[3] - dh) * 0.5, dw, dh], k)
                };
                let (a_fit, ka) = fit(aw, ah);
                let (b_fit, kb) = fit(bw, bh);
                Uniforms { rect, view, a_fit, b_fit, p0: [t as f32 % 1000.0, morph, hd.wob, glitch], p1: [ka, kb, 0.0, 0.0], ink }
            } else {
                Uniforms {
                    rect,
                    view,
                    a_fit: [bd.peers, 0.0, 0.0, 0.0],
                    b_fit: [0.0; 4],
                    p0: [t as f32 % 1000.0, bd.fill, bd.phase, bd.active],
                    p1: [splash, bd.active, 1.0, bd.done],
                    ink,
                }
            };
            queue.write_buffer(&slot.buf, 0, bytemuck::bytes_of(&u));
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..6, 0..1);
        });
    }
}
