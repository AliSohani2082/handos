//! M0 extra spike: does drawing the overlay on the GPU meet spike 2's CPU bar?
//!
//! Same scene as `overlay-spike` (two moving 21-point skeletons on a click-through layer-shell
//! surface, one output, frame callbacks), but drawn with wgpu: every line and joint is one
//! instanced quad shaded by its distance to the segment, so a frame is one draw call.
//!
//!   cargo run --release -p overlay-gpu -- [--trace FILE.jsonl | --synthetic] [--seconds 20]
//!       [--backend vulkan|gl] [--adapter low|high] [--every N] [--json out.json]
//!
//! Pass bar (spike 2's): RSS 40 MB or less, 3% of a core or less while animating, and CPU time
//! per frame 2 ms or less at p99.

#[path = "../../overlay/src/replay.rs"]
mod replay;

use std::ptr::NonNull;
use std::time::{Duration, Instant};

use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use replay::{Hands, Replay};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_registry,
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
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
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
const LINE_RGBA: [f32; 4] = [1.0, 1.0, 1.0, 220.0 / 255.0];
const JOINT_RGBA: [f32; 4] = [0.0, 200.0 / 255.0, 1.0, 1.0];
const FRAME_60HZ: Duration = Duration::from_micros(16_667);
const MAX_INST: usize = 4 * (CONNECTIONS.len() + 21); // room for four hands

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Inst {
    a: [f32; 2],
    b: [f32; 2],
    radius: f32,
    color: [f32; 4],
}

/// Lines first, then joints on top, in surface pixels.
fn build_instances(hands: &Hands, w: f32, h: f32, scale: f32, out: &mut Vec<Inst>) {
    out.clear();
    for hand in hands {
        let p = |i: usize| [hand[i][0] * w, hand[i][1] * h];
        for &(a, b) in &CONNECTIONS {
            out.push(Inst {
                a: p(a),
                b: p(b),
                radius: LINE_W * scale / 2.0,
                color: LINE_RGBA,
            });
        }
    }
    for hand in hands {
        for q in hand {
            let c = [q[0] * w, q[1] * h];
            out.push(Inst {
                a: c,
                b: c,
                radius: JOINT_R * scale,
                color: JOINT_RGBA,
            });
        }
    }
    out.truncate(MAX_INST);
}

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    bind: wgpu::BindGroup,
    insts: wgpu::Buffer,
    adapter_name: String,
    backend: String,
    present_mode: wgpu::PresentMode,
    alpha_mode: wgpu::CompositeAlphaMode,
}

impl Gpu {
    fn new(
        conn: &Connection,
        surface: &wl_surface::WlSurface,
        backend: wgpu::Backends,
        power: wgpu::PowerPreference,
    ) -> Gpu {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = backend;
        let instance = wgpu::Instance::new(desc);
        let display =
            NonNull::new(conn.backend().display_ptr().cast()).expect("wl_display pointer");
        let window = NonNull::new(surface.id().as_ptr().cast()).expect("wl_surface pointer");
        // SAFETY: the connection and the wl_surface live until the end of main, after `Gpu` is dropped.
        let wsurface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                    display,
                ))),
                raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
            })
        }
        .expect("create wgpu surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: power,
            compatible_surface: Some(&wsurface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("no GPU adapter for this surface");
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("overlay-gpu"),
            required_limits:
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .expect("request device");

        let caps = wsurface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let alpha_mode = [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
        ]
        .into_iter()
        .find(|m| caps.alpha_modes.contains(m))
        .unwrap_or(caps.alpha_modes[0]);
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: Default::default(),
            width: 1,
            height: 1,
            present_mode,
            desired_maximum_frame_latency: 1,
            alpha_mode,
            view_formats: vec![],
        };

        let shader = device.create_shader_module(wgpu::include_wgsl!("skeleton.wgsl"));
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bind_layout)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("skeleton"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Inst>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32, 3 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let insts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (MAX_INST * size_of::<Inst>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Gpu {
            surface: wsurface,
            device,
            queue,
            config,
            pipeline,
            globals,
            bind,
            insts,
            adapter_name: info.name,
            backend: format!("{:?}", info.backend),
            present_mode,
            alpha_mode,
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        self.config.width = w.max(1);
        self.config.height = h.max(1);
        self.surface.configure(&self.device, &self.config);
        let size = [
            self.config.width as f32,
            self.config.height as f32,
            0.0,
            0.0,
        ];
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::cast_slice(&size));
    }

    /// Records, submits and presents one frame. Returns false if no texture was available.
    fn render(&mut self, insts: &[Inst]) -> bool {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            _ => return false,
        };
        self.queue
            .write_buffer(&self.insts, 0, bytemuck::cast_slice(insts));
        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.set_vertex_buffer(0, self.insts.slice(..));
            pass.draw(0..4, 0..insts.len() as u32);
        }
        self.queue.submit([enc.finish()]);
        self.queue.present(frame);
        true
    }
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    layer: Option<LayerSurface>,
    gpu: Option<Gpu>,
    logical: (u32, u32),
    scale: u32,
    replay: Replay,
    start: Instant,
    hands: Hands,
    insts: Vec<Inst>,
    frame_us: Vec<f64>,
    frames: u64,
    skipped: u64,
    every: u64,
    /// With `--every N`, the next frame is drawn this long after a frame callback (a commit
    /// without a new buffer gets no callback from Hyprland, so the gap is timed instead).
    wake_at: Option<Instant>,
}

impl App {
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let (Some(layer), Some(gpu)) = (&self.layer, &mut self.gpu) else {
            return;
        };
        let t0 = Instant::now();
        self.replay.sample(self.start.elapsed(), &mut self.hands);
        let (w, h) = (gpu.config.width as f32, gpu.config.height as f32);
        build_instances(&self.hands, w, h, self.scale as f32, &mut self.insts);
        let surface = layer.wl_surface();
        surface.frame(qh, FrameCallbackData(surface.clone())); // must be requested before present commits
        if gpu.render(&self.insts) {
            self.frames += 1;
            self.frame_us.push(t0.elapsed().as_secs_f64() * 1e6);
        } else {
            self.skipped += 1;
            surface.commit();
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        s: &wl_surface::WlSurface,
        f: i32,
    ) {
        self.scale = f.max(1) as u32;
        s.set_buffer_scale(self.scale as i32);
        if let Some(gpu) = &mut self.gpu {
            gpu.resize(self.logical.0 * self.scale, self.logical.1 * self.scale);
        }
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        if self.every == 1 {
            self.draw(qh);
        } else {
            self.wake_at = Some(Instant::now() + FRAME_60HZ * (self.every as u32 - 1));
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
        _: &LayerSurface,
        cfg: LayerSurfaceConfigure,
        _: u32,
    ) {
        let first = self.logical == (0, 0);
        self.logical = cfg.new_size;
        if let Some(gpu) = &mut self.gpu {
            gpu.resize(self.logical.0 * self.scale, self.logical.1 * self.scale);
        }
        if first {
            self.draw(qh);
        }
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

/// Blocks until Wayland events arrive or `app.wake_at` passes (at most 100 ms), then dispatches.
fn wait_and_dispatch(
    conn: &Connection,
    queue: &mut wayland_client::EventQueue<App>,
    app: &mut App,
) {
    queue.flush().expect("flush");
    if let Some(guard) = queue.prepare_read() {
        let timeout_ms = app
            .wake_at
            .map(|t| {
                t.saturating_duration_since(Instant::now())
                    .as_micros()
                    .div_ceil(1000) as i32
            }) // round up: a 0 ms poll spins
            .unwrap_or(100)
            .min(100);
        let mut pfd = libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(&conn.backend().poll_fd()),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, owned by this frame.
        let n = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if n > 0 {
            guard.read().expect("read Wayland events");
        }
    }
    queue.dispatch_pending(app).expect("dispatch");
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
    s.sort_by(|a, b| a.total_cmp(b));
    s[((q * s.len() as f64) as usize).min(s.len() - 1)]
}

struct Args {
    trace: Option<String>,
    seconds: f64,
    json: Option<String>,
    backend: wgpu::Backends,
    power: wgpu::PowerPreference,
    every: u64,
}

fn parse_args() -> Args {
    let mut a = Args {
        trace: None,
        seconds: 20.0,
        json: None,
        backend: wgpu::Backends::VULKAN,
        power: wgpu::PowerPreference::LowPower,
        every: 1,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--trace" => a.trace = it.next(),
            "--synthetic" => a.trace = None,
            "--seconds" => a.seconds = it.next().and_then(|v| v.parse().ok()).expect("--seconds N"),
            "--json" => a.json = it.next(),
            "--every" => {
                a.every = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n| n > 0)
                    .expect("--every N (render every Nth display frame)")
            }
            "--backend" => {
                a.backend = match it.next().as_deref() {
                    Some("vulkan") => wgpu::Backends::VULKAN,
                    Some("gl") => wgpu::Backends::GL,
                    _ => panic!("--backend vulkan|gl"),
                }
            }
            "--adapter" => {
                a.power = match it.next().as_deref() {
                    Some("low") => wgpu::PowerPreference::LowPower,
                    Some("high") => wgpu::PowerPreference::HighPerformance,
                    _ => panic!("--adapter low|high"),
                }
            }
            "-h" | "--help" => {
                println!(
                    "overlay-gpu [--trace FILE.jsonl | --synthetic] [--seconds 20] [--backend vulkan|gl] [--adapter low|high] [--every N] [--json out.json]"
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
    let replay = match &args.trace {
        Some(p) => Replay::from_file(p).expect("read trace"),
        None => Replay::synthetic(),
    };

    let conn = Connection::connect_to_env().expect("no Wayland display");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("registry");
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("zwlr_layer_shell_v1");
    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        layer: None,
        gpu: None,
        logical: (0, 0),
        scale: 1,
        replay,
        start: Instant::now(),
        hands: Vec::new(),
        insts: Vec::with_capacity(MAX_INST),
        frame_us: Vec::with_capacity(1 << 16),
        frames: 0,
        skipped: 0,
        every: args.every,
        wake_at: None,
    };
    queue.roundtrip(&mut app).expect("roundtrip");

    let output = app.output_state.outputs().next().expect("no outputs");
    let info = app.output_state.info(&output);
    let name = info
        .as_ref()
        .and_then(|i| i.name.clone())
        .unwrap_or_default();
    app.scale = info.as_ref().map(|i| i.scale_factor).unwrap_or(1).max(1) as u32;
    let surface = compositor.create_surface(&qh);
    surface.set_buffer_scale(app.scale as i32);
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
    layer.commit();

    let rss_before_gpu = rss_kib();
    let gpu = Gpu::new(&conn, &surface, args.backend, args.power);
    eprintln!(
        "overlay-gpu on {name} for {:.0} s: {} via {}, {:?}, alpha {:?}. Click windows under it to check pass-through.",
        args.seconds, gpu.adapter_name, gpu.backend, gpu.present_mode, gpu.alpha_mode
    );
    app.gpu = Some(gpu);
    app.layer = Some(layer);
    app.start = Instant::now();

    let warmup = Duration::from_secs(2);
    let mut cpu0: Option<(Instant, f64, u64)> = None;
    let mut rss_max = 0u64;
    let mut last_sample = Instant::now();
    while app.start.elapsed().as_secs_f64() < args.seconds {
        wait_and_dispatch(&conn, &mut queue, &mut app);
        if app.wake_at.is_some_and(|t| Instant::now() >= t) {
            app.wake_at = None;
            app.draw(&qh);
        }
        if cpu0.is_none() && app.start.elapsed() >= warmup {
            cpu0 = Some((Instant::now(), proc_cpu_seconds(), app.frames));
            app.frame_us.clear();
        }
        if last_sample.elapsed() >= Duration::from_millis(250) {
            rss_max = rss_max.max(rss_kib());
            last_sample = Instant::now();
        }
    }
    let (t0, c0, f0) = cpu0.expect("run longer than the warm-up");
    let wall = t0.elapsed().as_secs_f64();
    let cpu_pct = (proc_cpu_seconds() - c0) / wall * 100.0;
    let fps = (app.frames - f0) as f64 / wall;
    let rss_end = rss_kib();
    rss_max = rss_max.max(rss_end);
    let gpu = app.gpu.as_ref().unwrap();

    let p50 = pct(&app.frame_us, 0.5) / 1000.0;
    let p95 = pct(&app.frame_us, 0.95) / 1000.0;
    let p99 = pct(&app.frame_us, 0.99) / 1000.0;
    let max = pct(&app.frame_us, 1.0) / 1000.0;
    let pass_rss = rss_max as f64 / 1024.0 <= 40.0;
    let pass_cpu = cpu_pct <= 3.0;
    let pass_frame = p99 <= 2.0;
    let ok = |b: bool| if b { "pass" } else { "FAIL" };
    println!(
        "\n== overlay-gpu: {} s measured after {} s warm-up",
        wall.round(),
        warmup.as_secs()
    );
    println!(
        "   {name}: {}x{} px, {} via {}, {:?}; {fps:.1} frames/s, skipped {}",
        gpu.config.width,
        gpu.config.height,
        gpu.adapter_name,
        gpu.backend,
        gpu.present_mode,
        app.skipped
    );
    println!(
        "   CPU time per frame (sample + record + submit + present) p50 {p50:.3} ms, p95 {p95:.3} ms, p99 {p99:.3} ms, max {max:.3} ms"
    );
    println!("   CPU {cpu_pct:.2}% of one core");
    println!(
        "   RSS max {:.1} MiB (end {:.1} MiB; before GPU init {:.1} MiB)",
        rss_max as f64 / 1024.0,
        rss_end as f64 / 1024.0,
        rss_before_gpu as f64 / 1024.0
    );
    let go = pass_rss && pass_cpu && pass_frame;
    println!(
        "   pass bar: RSS<=40MB {}  CPU<=3% {}  frames<=2ms (p99) {}  -> {}",
        ok(pass_rss),
        ok(pass_cpu),
        ok(pass_frame),
        if go { "GO" } else { "NO-GO" }
    );
    if let Some(path) = args.json {
        let j = serde_json::json!({
            "trace": args.trace, "output": name, "size": [gpu.config.width, gpu.config.height],
            "adapter": gpu.adapter_name, "backend": gpu.backend, "present_mode": format!("{:?}", gpu.present_mode),
            "seconds": wall, "every": app.every, "frames_per_s": fps, "skipped": app.skipped,
            "frame_ms": {"p50": p50, "p95": p95, "p99": p99, "max": max, "n": app.frame_us.len()},
            "cpu_pct": cpu_pct, "rss_max_mib": rss_max as f64 / 1024.0, "rss_end_mib": rss_end as f64 / 1024.0,
            "rss_before_gpu_mib": rss_before_gpu as f64 / 1024.0, "go": go,
        });
        std::fs::write(path, serde_json::to_string_pretty(&j).unwrap()).expect("write json");
    }
    drop(app.gpu.take()); // the wgpu surface must go before the wl_surface and connection
    drop(region);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances_cover_every_bone_then_every_joint() {
        let hands: Hands = vec![std::array::from_fn(|i| [i as f32 / 20.0, 0.5]); 2];
        let mut out = Vec::new();
        build_instances(&hands, 1000.0, 500.0, 2.0, &mut out);
        assert_eq!(out.len(), 2 * (CONNECTIONS.len() + 21));
        let lines = &out[..2 * CONNECTIONS.len()];
        let joints = &out[2 * CONNECTIONS.len()..];
        assert!(
            lines
                .iter()
                .all(|i| i.radius == LINE_W && i.color == LINE_RGBA)
        );
        assert!(
            joints
                .iter()
                .all(|i| i.a == i.b && i.radius == JOINT_R * 2.0 && i.color == JOINT_RGBA)
        );
        assert_eq!(lines[0].a, [0.0, 250.0]);
        assert_eq!(lines[0].b, [50.0, 250.0]);
    }

    #[test]
    fn instances_are_capped_at_the_buffer_size() {
        let hands: Hands = vec![[[0.5, 0.5]; 21]; 9];
        let mut out = Vec::new();
        build_instances(&hands, 10.0, 10.0, 1.0, &mut out);
        assert_eq!(out.len(), MAX_INST);
    }

    #[test]
    fn shader_has_both_entry_points() {
        let src = include_str!("skeleton.wgsl");
        assert!(src.contains("fn vs") && src.contains("fn fs"));
    }
}
