//! Spike 2: what do a click-through layer-shell overlay and CPU rendering cost on Hyprland?
//!
//! Draws a hand skeleton from a replayed trace (or a synthetic one) on every output, at display
//! rate, redrawing only damaged rectangles. Reports frame time, CPU and RSS.
//!
//!   cargo run --release -p overlay-spike -- [--trace FILE.jsonl | --synthetic] [--seconds 30] [--json out.json]
//!
//! Pass bar: 40 MB RSS or less, 3% of a core or less while animating, frames of 2 ms or less.

mod replay;

use std::time::{Duration, Instant};

use replay::{Hands, Replay};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{wp_fractional_scale_manager_v1, wp_fractional_scale_v1},
    viewporter::client::{wp_viewport, wp_viewporter},
};

const CONNECTIONS: [(usize, usize); 21] = [
    (0, 1),
    (1, 2),
    (2, 3),
    (3, 4),
    (0, 5),
    (5, 6),
    (6, 7),
    (7, 8),
    (5, 9),
    (9, 10),
    (10, 11),
    (11, 12),
    (9, 13),
    (13, 14),
    (14, 15),
    (15, 16),
    (13, 17),
    (17, 18),
    (18, 19),
    (19, 20),
    (0, 17),
];
const LINE_W: f32 = 3.0;
const JOINT_R: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl Rect {
    fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
        match (a, b) {
            (Some(a), Some(b)) => Some(Rect {
                x0: a.x0.min(b.x0),
                y0: a.y0.min(b.y0),
                x1: a.x1.max(b.x1),
                y1: a.y1.max(b.y1),
            }),
            (a, None) => a,
            (None, b) => b,
        }
    }

    fn clamp(self, w: i32, h: i32) -> Option<Rect> {
        let r = Rect {
            x0: self.x0.max(0),
            y0: self.y0.max(0),
            x1: self.x1.min(w),
            y1: self.y1.min(h),
        };
        (r.x0 < r.x1 && r.y0 < r.y1).then_some(r)
    }

    fn area(self) -> i64 {
        (self.x1 - self.x0) as i64 * (self.y1 - self.y0) as i64
    }

    fn overlaps(self, o: Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
}

/// Merge overlapping rectangles so no pixel is copied twice; collapse long lists into one box.
fn merged(mut v: Vec<Rect>) -> Vec<Rect> {
    let mut k = 0;
    while k < v.len() {
        if let Some(j) = (k + 1..v.len()).find(|&j| v[k].overlaps(v[j])) {
            let other = v.swap_remove(j);
            v[k] = Rect::union(Some(v[k]), Some(other)).unwrap();
            k = 0;
        } else {
            k += 1;
        }
    }
    if v.len() > 6 {
        let all = v.iter().copied().fold(None, |a, r| Rect::union(a, Some(r)));
        return all.into_iter().collect();
    }
    v
}

struct Buf {
    buffer: Buffer,
    stale: Vec<Rect>,
}

struct Out {
    name: String,
    layer: LayerSurface,
    _input_region: Region,
    viewport: Option<wp_viewport::WpViewport>,
    _frac: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    logical: (u32, u32),
    scale120: u32,
    configured: bool,
    pool: SlotPool,
    bufs: Vec<Buf>,
    pixmap: Option<Pixmap>,
    prev: Vec<Rect>,
    /// Hands map to one pointer space, so by default only the first output shows them.
    draws_hands: bool,
    /// Plan fallback: render below output resolution and let the viewport scale it up.
    render_scale: f64,
    /// No frame callback pending: nothing on this output is changing.
    idle: bool,
    callbacks: u64,
    no_free_buffer: u64,
    damaged_px: i64,
}

impl Out {
    fn physical(&self) -> (u32, u32) {
        let s = self.scale120 as f64 / 120.0 * self.render_scale;
        (
            (self.logical.0 as f64 * s).round() as u32,
            (self.logical.1 as f64 * s).round() as u32,
        )
    }
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    outs: Vec<Out>,
    replay: Replay,
    start: Instant,
    frame_us: Vec<f64>,
    hand_scratch: Hands,
    /// Time split, summed after warm-up: (sample+render, copy, wayland requests) in microseconds.
    split_us: [f64; 3],
}

impl App {
    fn out_index(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.outs
            .iter()
            .position(|o| o.layer.wl_surface() == surface)
    }

    fn draw(&mut self, i: usize, qh: &QueueHandle<Self>) {
        let t_start = Instant::now();
        let t = self.start.elapsed();
        self.replay.sample(t, &mut self.hand_scratch);
        let out = &mut self.outs[i];
        if !out.configured {
            return;
        }
        let (pw, ph) = out.physical();
        let surface = out.layer.wl_surface().clone();

        // (Re)allocate on first use or after a resize or scale change.
        if out
            .pixmap
            .as_ref()
            .is_none_or(|p| p.width() != pw || p.height() != ph)
        {
            out.pixmap = Pixmap::new(pw, ph);
            out.pool
                .resize((pw * ph * 4 * 2) as usize)
                .expect("pool resize");
            out.bufs = (0..2)
                .map(|_| Buf {
                    buffer: out
                        .pool
                        .create_buffer(
                            pw as i32,
                            ph as i32,
                            pw as i32 * 4,
                            wl_shm::Format::Argb8888,
                        )
                        .expect("buffer")
                        .0,
                    stale: vec![Rect {
                        x0: 0,
                        y0: 0,
                        x1: pw as i32,
                        y1: ph as i32,
                    }],
                })
                .collect();
            out.prev.clear();
            if let Some(vp) = &out.viewport {
                vp.set_destination(out.logical.0 as i32, out.logical.1 as i32);
            }
        }
        let pixmap = out.pixmap.as_mut().unwrap();
        let s = (out.scale120 as f64 / 120.0 * out.render_scale) as f32;

        // Clear what was drawn last time, draw the skeleton now, and remember its bounds (one box per hand).
        for &r in &out.prev {
            clear_rect(pixmap, r);
        }
        let no_hands = Hands::new();
        let new = draw_hands(
            pixmap,
            if out.draws_hands {
                &self.hand_scratch
            } else {
                &no_hands
            },
            s,
        );
        let t_rendered = Instant::now();
        let damage = merged(out.prev.drain(..).chain(new.iter().copied()).collect());
        out.prev = new;
        if damage.is_empty() {
            out.idle = true; // nothing changes here: stop asking for frames
            return;
        }
        out.idle = false;

        // Repair the free buffer: everything damaged since it was last shown, plus this frame.
        let Some(bi) =
            (0..out.bufs.len()).find(|&k| out.bufs[k].buffer.canvas(&mut out.pool).is_some())
        else {
            out.no_free_buffer += 1;
            surface.frame(qh, FrameCallbackData(surface.clone()));
            surface.commit();
            return;
        };
        let mut stale = std::mem::take(&mut out.bufs[bi].stale);
        stale.extend_from_slice(&damage);
        let repair: Vec<Rect> = merged(stale)
            .into_iter()
            .filter_map(|r| r.clamp(pw as i32, ph as i32))
            .collect();
        for (k, b) in out.bufs.iter_mut().enumerate() {
            if k != bi {
                b.stale.extend_from_slice(&damage);
                b.stale = merged(std::mem::take(&mut b.stale));
            }
        }
        let canvas = out.bufs[bi].buffer.canvas(&mut out.pool).unwrap();
        for &r in &repair {
            copy_rect(pixmap, canvas, r);
        }
        let t_copied = Instant::now();
        for &r in &repair {
            surface.damage_buffer(r.x0, r.y0, r.x1 - r.x0, r.y1 - r.y0);
            out.damaged_px += r.area();
        }
        surface.frame(qh, FrameCallbackData(surface.clone()));
        out.bufs[bi].buffer.attach_to(&surface).expect("attach");
        surface.commit();
        out.callbacks += 1;
        if t > Duration::from_secs(2) {
            let end = Instant::now();
            self.frame_us.push((end - t_start).as_secs_f64() * 1e6);
            self.split_us[0] += (t_rendered - t_start).as_secs_f64() * 1e6;
            self.split_us[1] += (t_copied - t_rendered).as_secs_f64() * 1e6;
            self.split_us[2] += (end - t_copied).as_secs_f64() * 1e6;
        }
    }
}

fn clear_rect(p: &mut Pixmap, r: Rect) {
    let Some(r) = r.clamp(p.width() as i32, p.height() as i32) else {
        return;
    };
    let w = p.width() as usize;
    let data = p.data_mut();
    for y in r.y0 as usize..r.y1 as usize {
        data[(y * w + r.x0 as usize) * 4..(y * w + r.x1 as usize) * 4].fill(0);
    }
}

/// tiny-skia stores premultiplied RGBA; wl_shm ARGB8888 is premultiplied BGRA in memory.
fn copy_rect(p: &Pixmap, canvas: &mut [u8], r: Rect) {
    let w = p.width() as usize;
    let src = p.data();
    for y in r.y0 as usize..r.y1 as usize {
        let a = (y * w + r.x0 as usize) * 4;
        let b = (y * w + r.x1 as usize) * 4;
        let (dst, _) = canvas[a..b].as_chunks_mut::<4>();
        let (src, _) = src[a..b].as_chunks::<4>();
        for (d, s) in dst.iter_mut().zip(src) {
            *d = [s[2], s[1], s[0], s[3]];
        }
    }
}

fn draw_hands(p: &mut Pixmap, hands: &Hands, s: f32) -> Vec<Rect> {
    let (w, h) = (p.width() as f32, p.height() as f32);
    let mut out = Vec::new();
    let mut line = Paint::default();
    line.set_color(Color::from_rgba8(255, 255, 255, 220));
    line.anti_alias = true;
    let mut joint = Paint::default();
    joint.set_color(Color::from_rgba8(0, 200, 255, 255));
    joint.anti_alias = true;
    let stroke = Stroke {
        width: LINE_W * s,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    let pad = ((JOINT_R + LINE_W) * s).ceil() as i32 + 2;
    for hand in hands {
        let pts: Vec<(f32, f32)> = hand.iter().map(|q| (q[0] * w, q[1] * h)).collect();
        let mut pb = PathBuilder::new();
        for &(a, b) in &CONNECTIONS {
            pb.move_to(pts[a].0, pts[a].1);
            pb.line_to(pts[b].0, pts[b].1);
        }
        if let Some(path) = pb.finish() {
            p.stroke_path(&path, &line, &stroke, Transform::identity(), None);
        }
        let mut jb = PathBuilder::new();
        for &(x, y) in &pts {
            jb.push_circle(x, y, JOINT_R * s);
        }
        if let Some(path) = jb.finish() {
            p.fill_path(
                &path,
                &joint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        let mut bounds: Option<Rect> = None;
        for &(x, y) in &pts {
            let r = Rect {
                x0: x as i32 - pad,
                y0: y as i32 - pad,
                x1: x as i32 + pad,
                y1: y as i32 + pad,
            };
            bounds = Rect::union(bounds, Some(r));
        }
        out.extend(bounds.and_then(|r| r.clamp(w as i32, h as i32)));
    }
    out
}

// ------------------------------------------------------------------ Wayland plumbing

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(i) = self.out_index(surface) {
            self.draw(i, qh);
        }
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}
    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        cfg: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(i) = self.outs.iter().position(|o| &o.layer == layer) else {
            return;
        };
        let first = !self.outs[i].configured;
        self.outs[i].logical = cfg.new_size;
        self.outs[i].configured = true;
        if first {
            self.draw(i, qh);
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);

/// User data for protocol objects that send no events we care about.
struct NoEvents;
impl Dispatch2<wp_viewporter::WpViewporter, App> for NoEvents {
    fn event(
        &self,
        _: &mut App,
        _: &wp_viewporter::WpViewporter,
        _: wp_viewporter::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}
impl Dispatch2<wp_viewport::WpViewport, App> for NoEvents {
    fn event(
        &self,
        _: &mut App,
        _: &wp_viewport::WpViewport,
        _: wp_viewport::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}
impl Dispatch2<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, App> for NoEvents {
    fn event(
        &self,
        _: &mut App,
        _: &wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        _: wp_fractional_scale_manager_v1::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

/// Fractional scale of one output's surface, in 120ths.
struct FracScale(usize);
impl Dispatch2<wp_fractional_scale_v1::WpFractionalScaleV1, App> for FracScale {
    fn event(
        &self,
        app: &mut App,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        ev: wp_fractional_scale_v1::Event,
        _: &Connection,
        qh: &QueueHandle<App>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = ev {
            let out = &mut app.outs[self.0];
            if out.scale120 != scale {
                eprintln!("{}: preferred scale {:.3}", out.name, scale as f64 / 120.0);
                out.scale120 = scale;
                if out.configured {
                    app.draw(self.0, qh);
                }
            }
        }
    }
}

// ------------------------------------------------------------------ measurement

fn proc_cpu_seconds() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let after = stat.rsplit_once(')').map(|x| x.1).unwrap_or("");
    let f: Vec<&str> = after.split_whitespace().collect();
    let ticks: f64 = f.get(11).and_then(|v| v.parse().ok()).unwrap_or(0.0)
        + f.get(12).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    ticks / unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64
}

fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}

fn pct(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((q * s.len() as f64) as usize).min(s.len() - 1)]
}

struct Args {
    trace: Option<String>,
    seconds: f64,
    json: Option<String>,
    all_outputs: bool,
    render_scale: f64,
}

fn parse_args() -> Args {
    let mut a = Args {
        trace: None,
        seconds: 30.0,
        json: None,
        all_outputs: false,
        render_scale: 1.0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--trace" => a.trace = it.next(),
            "--synthetic" => a.trace = None,
            "--seconds" => a.seconds = it.next().and_then(|v| v.parse().ok()).expect("--seconds N"),
            "--json" => a.json = it.next(),
            "--all-outputs" => a.all_outputs = true,
            "--render-scale" => {
                a.render_scale = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--render-scale F")
            }
            "-h" | "--help" => {
                println!(
                    "overlay-spike [--trace FILE.jsonl | --synthetic] [--seconds 30] [--all-outputs] [--render-scale 0.5] [--json out.json]"
                );
                std::process::exit(0);
            }
            other => panic!("unknown argument {other}"),
        }
    }
    a
}

fn main() {
    let args = parse_args();
    let rss_before_wayland = rss_kib();
    let replay = match &args.trace {
        Some(p) => Replay::from_file(p).expect("read trace"),
        None => Replay::synthetic(),
    };

    let conn = Connection::connect_to_env().expect("no Wayland display");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("registry");
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("zwlr_layer_shell_v1");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm");
    let viewporter: Option<wp_viewporter::WpViewporter> = globals.bind(&qh, 1..=1, NoEvents).ok();
    let frac_mgr: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1> =
        globals.bind(&qh, 1..=1, NoEvents).ok();

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        outs: Vec::new(),
        replay,
        start: Instant::now(),
        frame_us: Vec::with_capacity(1 << 16),
        hand_scratch: Vec::new(),
        split_us: [0.0; 3],
    };
    queue.roundtrip(&mut app).expect("roundtrip");

    let outputs: Vec<_> = app.output_state.outputs().collect();
    for (i, output) in outputs.into_iter().enumerate() {
        let info = app.output_state.info(&output);
        let name = info
            .as_ref()
            .and_then(|i| i.name.clone())
            .unwrap_or_else(|| format!("output-{i}"));
        let int_scale = info.as_ref().map(|i| i.scale_factor).unwrap_or(1).max(1) as u32;
        let surface = compositor.create_surface(&qh);
        let layer = layer_shell.create_layer_surface(
            &qh,
            surface.clone(),
            Layer::Overlay,
            Some("handos-spike"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        let region = Region::new(&compositor).expect("region"); // empty: every click passes through
        layer.set_input_region(Some(region.wl_region()));
        let viewport = viewporter
            .as_ref()
            .map(|v| v.get_viewport(&surface, &qh, NoEvents));
        let frac = frac_mgr
            .as_ref()
            .map(|m| m.get_fractional_scale(&surface, &qh, FracScale(i)));
        if viewport.is_none() || frac.is_none() {
            surface.set_buffer_scale(int_scale as i32);
        }
        layer.commit();
        app.outs.push(Out {
            name,
            layer,
            _input_region: region,
            viewport,
            _frac: frac,
            logical: (0, 0),
            scale120: if frac_mgr.is_some() {
                120
            } else {
                int_scale * 120
            },
            configured: false,
            pool: SlotPool::new(4096, &app.shm).expect("pool"),
            bufs: Vec::new(),
            pixmap: None,
            prev: Vec::new(),
            draws_hands: args.all_outputs || i == 0,
            render_scale: if viewporter.is_some() {
                args.render_scale
            } else {
                1.0
            },
            idle: false,
            callbacks: 0,
            no_free_buffer: 0,
            damaged_px: 0,
        });
    }
    eprintln!(
        "overlay on {} output(s) for {:.0} s; viewporter {}, fractional-scale {}. Click windows under it to check pass-through.",
        app.outs.len(),
        args.seconds,
        viewporter.is_some(),
        frac_mgr.is_some()
    );

    let warmup = Duration::from_secs(2);
    let mut cpu0: Option<(Instant, f64)> = None;
    let mut rss_max = 0u64;
    let mut last_sample = Instant::now();
    let mut cb0: Vec<u64> = Vec::new();
    while app.start.elapsed().as_secs_f64() < args.seconds {
        queue.blocking_dispatch(&mut app).expect("dispatch");
        if cpu0.is_none() && app.start.elapsed() >= warmup {
            cpu0 = Some((Instant::now(), proc_cpu_seconds()));
            cb0 = app.outs.iter().map(|o| o.callbacks).collect();
        }
        if last_sample.elapsed() >= Duration::from_millis(250) {
            rss_max = rss_max.max(rss_kib());
            last_sample = Instant::now();
        }
    }
    let (t0, c0) = cpu0.expect("run longer than the warm-up");
    let wall = t0.elapsed().as_secs_f64();
    let cpu_pct = (proc_cpu_seconds() - c0) / wall * 100.0;
    let rss_end = rss_kib();
    rss_max = rss_max.max(rss_end);

    let p50 = pct(&app.frame_us, 0.5) / 1000.0;
    let p95 = pct(&app.frame_us, 0.95) / 1000.0;
    let p99 = pct(&app.frame_us, 0.99) / 1000.0;
    let max = pct(&app.frame_us, 1.0) / 1000.0;
    let pass_rss = rss_max as f64 / 1024.0 <= 40.0;
    let pass_cpu = cpu_pct <= 3.0;
    let pass_frame = p99 <= 2.0;
    println!(
        "\n== overlay spike: {} s measured after {} s warm-up",
        wall.round(),
        warmup.as_secs()
    );
    for (k, o) in app.outs.iter().enumerate() {
        let (pw, ph) = o.physical();
        let hz = (o.callbacks - cb0.get(k).copied().unwrap_or(0)) as f64 / wall;
        let avg_damage =
            o.damaged_px as f64 / o.callbacks.max(1) as f64 / (pw as f64 * ph as f64) * 100.0;
        println!(
            "   {}{}: logical {}x{}, scale {:.3}, buffers {}x{}, {:.1} frames/s, avg damage {:.2}% of output, no-free-buffer {}",
            o.name,
            if o.draws_hands { " (hands)" } else { " (idle)" },
            o.logical.0,
            o.logical.1,
            o.scale120 as f64 / 120.0,
            pw,
            ph,
            hz,
            avg_damage,
            o.no_free_buffer
        );
    }
    println!(
        "   frame time (render + copy) p50 {p50:.3} ms, p95 {p95:.3} ms, p99 {p99:.3} ms, max {max:.3} ms over {} frames",
        app.frame_us.len()
    );
    let n = app.frame_us.len().max(1) as f64;
    println!(
        "   mean split: clear+render {:.3} ms, copy to shm {:.3} ms, wayland requests {:.3} ms",
        app.split_us[0] / n / 1000.0,
        app.split_us[1] / n / 1000.0,
        app.split_us[2] / n / 1000.0
    );
    println!("   CPU {cpu_pct:.2}% of one core");
    println!(
        "   RSS max {:.1} MiB (end {:.1} MiB; before Wayland {:.1} MiB)",
        rss_max as f64 / 1024.0,
        rss_end as f64 / 1024.0,
        rss_before_wayland as f64 / 1024.0
    );
    println!(
        "   pass bar: RSS<=40MB {}  CPU<=3% {}  frames<=2ms (p99) {}  -> {}",
        ok(pass_rss),
        ok(pass_cpu),
        ok(pass_frame),
        if pass_rss && pass_cpu && pass_frame {
            "GO"
        } else {
            "NO-GO"
        }
    );
    if let Some(path) = args.json {
        let outs: Vec<_> = app
            .outs
            .iter()
            .enumerate()
            .map(|(k, o)| {
                serde_json::json!({
                    "name": o.name, "logical": [o.logical.0, o.logical.1], "scale": o.scale120 as f64 / 120.0,
                    "frames_per_s": (o.callbacks - cb0.get(k).copied().unwrap_or(0)) as f64 / wall,
                    "no_free_buffer": o.no_free_buffer,
                })
            })
            .collect();
        let j = serde_json::json!({
            "trace": args.trace, "all_outputs": args.all_outputs, "render_scale": args.render_scale, "seconds": wall, "outputs": outs,
            "frame_ms": {"p50": p50, "p95": p95, "p99": p99, "max": max, "n": app.frame_us.len()},
            "cpu_pct": cpu_pct, "rss_max_mib": rss_max as f64 / 1024.0, "rss_end_mib": rss_end as f64 / 1024.0,
            "go": pass_rss && pass_cpu && pass_frame,
        });
        std::fs::write(path, serde_json::to_string_pretty(&j).unwrap()).expect("write json");
    }
}

fn ok(b: bool) -> &'static str {
    if b { "ok" } else { "FAIL" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_and_clamp() {
        let a = Rect {
            x0: 0,
            y0: 0,
            x1: 10,
            y1: 10,
        };
        let b = Rect {
            x0: 5,
            y0: -5,
            x1: 20,
            y1: 8,
        };
        assert_eq!(
            Rect::union(Some(a), Some(b)),
            Some(Rect {
                x0: 0,
                y0: -5,
                x1: 20,
                y1: 10
            })
        );
        assert_eq!(Rect::union(None, Some(b)), Some(b));
        assert_eq!(
            b.clamp(15, 15),
            Some(Rect {
                x0: 5,
                y0: 0,
                x1: 15,
                y1: 8
            })
        );
        assert_eq!(
            Rect {
                x0: 20,
                y0: 0,
                x1: 30,
                y1: 5
            }
            .clamp(15, 15),
            None
        );
    }

    #[test]
    fn merged_joins_overlaps_and_keeps_disjoint_boxes() {
        let a = Rect {
            x0: 0,
            y0: 0,
            x1: 10,
            y1: 10,
        };
        let b = Rect {
            x0: 5,
            y0: 5,
            x1: 15,
            y1: 15,
        };
        let c = Rect {
            x0: 100,
            y0: 100,
            x1: 110,
            y1: 110,
        };
        let m = merged(vec![a, c, b]);
        assert_eq!(m.len(), 2);
        assert!(
            m.contains(&Rect {
                x0: 0,
                y0: 0,
                x1: 15,
                y1: 15
            }) && m.contains(&c)
        );
        let many: Vec<Rect> = (0..10)
            .map(|k| Rect {
                x0: k * 20,
                y0: 0,
                x1: k * 20 + 5,
                y1: 5,
            })
            .collect();
        assert_eq!(
            merged(many),
            vec![Rect {
                x0: 0,
                y0: 0,
                x1: 185,
                y1: 5
            }]
        );
    }

    #[test]
    fn copy_swizzles_rgba_to_bgra_inside_the_rect_only() {
        let mut p = Pixmap::new(4, 2).unwrap();
        p.data_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|px| *px = [1, 2, 3, 4]);
        let mut canvas = vec![0u8; 4 * 2 * 4];
        copy_rect(
            &p,
            &mut canvas,
            Rect {
                x0: 1,
                y0: 1,
                x1: 3,
                y1: 2,
            },
        );
        let row1 = &canvas[16..32];
        assert_eq!(&row1[0..4], &[0, 0, 0, 0]);
        assert_eq!(&row1[4..8], &[3, 2, 1, 4]);
        assert_eq!(&row1[8..12], &[3, 2, 1, 4]);
        assert!(canvas[..16].iter().all(|&b| b == 0));
    }

    #[test]
    fn drawn_bounds_cover_every_painted_pixel() {
        let mut p = Pixmap::new(400, 300).unwrap();
        let mut hands = Vec::new();
        Replay::synthetic().sample(Duration::from_millis(700), &mut hands);
        let rects = draw_hands(&mut p, &hands, 1.0);
        assert_eq!(rects.len(), 2, "one box per hand");
        let w = p.width() as i32;
        for (k, px) in p.data().as_chunks::<4>().0.iter().enumerate() {
            if px[3] != 0 {
                let (x, y) = (k as i32 % w, k as i32 / w);
                assert!(
                    rects
                        .iter()
                        .any(|r| x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1),
                    "pixel {x},{y} outside {rects:?}"
                );
            }
        }
        for r in rects {
            clear_rect(&mut p, r);
        }
        assert!(
            p.data().iter().all(|&b| b == 0),
            "clearing the bounds removes the whole drawing"
        );
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// cargo test --release -p overlay-spike bench_draw -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_draw() {
        let mut p = Pixmap::new(1920, 1080).unwrap();
        let mut hands = Vec::new();
        let r = Replay::synthetic();
        let n = 300;
        let (mut stroke_t, mut fill_t, mut clear_t) = (0.0, 0.0, 0.0);
        let line = Paint {
            anti_alias: true,
            ..Paint::default()
        };
        let stroke = Stroke {
            width: LINE_W,
            line_cap: LineCap::Round,
            ..Stroke::default()
        };
        for k in 0..n {
            r.sample(Duration::from_millis(k * 16), &mut hands);
            let t0 = Instant::now();
            for hand in &hands {
                let mut pb = PathBuilder::new();
                for &(a, b) in &CONNECTIONS {
                    pb.move_to(hand[a][0] * 1920.0, hand[a][1] * 1080.0);
                    pb.line_to(hand[b][0] * 1920.0, hand[b][1] * 1080.0);
                }
                p.stroke_path(
                    &pb.finish().unwrap(),
                    &line,
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
            let t1 = Instant::now();
            for hand in &hands {
                let mut jb = PathBuilder::new();
                for q in hand {
                    jb.push_circle(q[0] * 1920.0, q[1] * 1080.0, JOINT_R);
                }
                p.fill_path(
                    &jb.finish().unwrap(),
                    &line,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            let t2 = Instant::now();
            clear_rect(
                &mut p,
                Rect {
                    x0: 0,
                    y0: 0,
                    x1: 1920,
                    y1: 1080,
                },
            );
            let t3 = Instant::now();
            stroke_t += (t1 - t0).as_secs_f64();
            fill_t += (t2 - t1).as_secs_f64();
            clear_t += (t3 - t2).as_secs_f64();
        }
        let ms = |x: f64| x / n as f64 * 1000.0;
        println!(
            "per frame, two hands: stroke {:.3} ms, joints {:.3} ms, full clear {:.3} ms",
            ms(stroke_t),
            ms(fill_t),
            ms(clear_t)
        );
    }
}
