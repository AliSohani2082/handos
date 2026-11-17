//! Spike 3: do the wlr virtual pointer and virtual keyboard give exact clicks and correct text,
//! Persian included, in real applications?
//!
//!   input-spike clicks [--cols 6 --rows 4] [--json out.json]
//!       Covers every output with a target surface, clicks each target with a virtual pointer
//!       bound to that output, and measures where the click landed. A click is only sent once
//!       the pointer is confirmed over the spike's own surface, so it never reaches another app.
//!   input-spike type --method keymap|paste --text TEXT [--enter] [--paste-keys ctrl-v|ctrl-shift-v]
//!                    [--expect-class SUBSTRING] [--delay-ms 0]
//!       Types TEXT into the focused window. With --expect-class, refuses unless Hyprland's
//!       active window class contains SUBSTRING.
//!
//! Pass bar: every click within 2 px; both sentences correct in all four applications by at
//! least one method.

mod keymap;

use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use keymap::{CONTROL, Keymap, SHIFT};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, EventQueue, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1, zwlr_virtual_pointer_v1,
};

const BTN_LEFT: u32 = 0x110;

struct TargetOut {
    name: String,
    output: wl_output::WlOutput,
    logical_position: (i32, i32),
    layer: LayerSurface,
    size: (u32, u32),
    configured: bool,
    pool: SlotPool,
}

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    outs: Vec<TargetOut>,
    pointer: Option<wl_pointer::WlPointer>,
    /// Surface under the pointer and the latest position on it.
    focus: Option<(wl_surface::WlSurface, (f64, f64))>,
    presses: Vec<(wl_surface::WlSurface, (f64, f64))>,
}

impl App {
    fn new(globals: &wayland_client::globals::GlobalList, qh: &QueueHandle<App>) -> App {
        App {
            registry_state: RegistryState::new(globals),
            seat_state: SeatState::new(globals, qh),
            output_state: OutputState::new(globals, qh),
            shm: Shm::bind(globals, qh).expect("wl_shm"),
            outs: Vec::new(),
            pointer: None,
            focus: None,
            presses: Vec::new(),
        }
    }

    fn seat(&self) -> wl_seat::WlSeat {
        self.seat_state.seats().next().expect("no seat")
    }

    /// Paint a dim backdrop with a ring at every target, so the run is visible.
    fn paint(&mut self, i: usize, targets: &[(i32, i32)]) {
        let o = &mut self.outs[i];
        let (w, h) = (o.size.0 as i32, o.size.1 as i32);
        let (buffer, canvas) = o
            .pool
            .create_buffer(w, h, w * 4, wl_shm::Format::Argb8888)
            .expect("buffer");
        for (k, px) in canvas.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (k as i32 % w, k as i32 / w);
            let ring = targets.iter().any(|&(tx, ty)| {
                let d2 = (x - tx).pow(2) + (y - ty).pow(2);
                (36..=64).contains(&d2)
                    || (x == tx && (y - ty).abs() <= 12)
                    || (y == ty && (x - tx).abs() <= 12)
            });
            *px = if ring {
                [0, 0, 255, 255]
            } else {
                [0, 0, 0, 0x60]
            }; // BGRA, premultiplied
        }
        let surface = o.layer.wl_surface();
        surface.damage_buffer(0, 0, w, h);
        buffer.attach_to(surface).expect("attach");
        o.layer.commit();
    }
}

fn targets_for(w: u32, h: u32, cols: u32, rows: u32) -> Vec<(i32, i32)> {
    let mut t: Vec<(i32, i32)> = (0..rows)
        .flat_map(|r| {
            (0..cols).map(move |c| {
                (
                    ((c * 2 + 1) * w / (cols * 2)) as i32,
                    ((r * 2 + 1) * h / (rows * 2)) as i32,
                )
            })
        })
        .collect();
    // Corners, 3 px in: the extremes of the absolute-motion range.
    let (w, h) = (w as i32, h as i32);
    t.extend([(3, 3), (w - 4, 3), (3, h - 4), (w - 4, h - 4)]);
    t
}

fn now_ms(start: Instant) -> u32 {
    start.elapsed().as_millis() as u32
}

/// Dispatch until `done` holds or `timeout` passes.
fn pump(
    queue: &mut EventQueue<App>,
    app: &mut App,
    timeout: Duration,
    done: impl Fn(&App) -> bool,
) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        queue.roundtrip(app).expect("roundtrip");
        if done(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    done(app)
}

fn cmd_clicks(args: &[String]) {
    let cols: u32 = flag(args, "--cols")
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let rows: u32 = flag(args, "--rows")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let json_path = flag(args, "--json");

    let conn = Connection::connect_to_env().expect("no Wayland display");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("registry");
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("layer shell");
    let vp_mgr: zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1 = globals
        .bind(&qh, 1..=2, NoEvents)
        .expect("zwlr_virtual_pointer_manager_v1 not offered");
    let mut app = App::new(&globals, &qh);
    queue.roundtrip(&mut app).unwrap();
    queue.roundtrip(&mut app).unwrap();

    for output in app.output_state.outputs().collect::<Vec<_>>() {
        let info = app.output_state.info(&output).expect("output info");
        let surface = compositor.create_surface(&qh);
        let layer = layer_shell.create_layer_surface(
            &qh,
            surface,
            Layer::Overlay,
            Some("handos-spike-targets"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();
        app.outs.push(TargetOut {
            name: info.name.clone().unwrap_or_default(),
            output,
            logical_position: info.logical_position.unwrap_or((0, 0)),
            layer,
            size: (0, 0),
            configured: false,
            pool: SlotPool::new(4096, &app.shm).expect("pool"),
        });
    }
    assert!(
        pump(&mut queue, &mut app, Duration::from_secs(2), |a| a
            .outs
            .iter()
            .all(|o| o.configured)),
        "surfaces not configured"
    );
    let all_targets: Vec<Vec<(i32, i32)>> = app
        .outs
        .iter()
        .map(|o| targets_for(o.size.0, o.size.1, cols, rows))
        .collect();
    for (i, t) in all_targets.iter().enumerate() {
        app.paint(i, t);
    }
    queue.roundtrip(&mut app).unwrap();
    std::thread::sleep(Duration::from_millis(300)); // let the compositor map the surfaces

    let seat = app.seat();
    let version = vp_mgr.version();
    let start = Instant::now();
    let mut report = Vec::new();
    let mut worst = 0.0f64;
    let mut misses = 0;
    for (i, targets) in all_targets.iter().enumerate() {
        let (w, h) = app.outs[i].size;
        let surface = app.outs[i].layer.wl_surface().clone();
        let (vp, extent, offset) = if version >= 2 {
            (
                vp_mgr.create_virtual_pointer_with_output(
                    Some(&seat),
                    Some(&app.outs[i].output),
                    &qh,
                    NoEvents,
                ),
                (w, h),
                (0, 0),
            )
        } else {
            // v1: absolute motion spans the whole layout.
            let lw = app
                .outs
                .iter()
                .map(|o| o.logical_position.0 + o.size.0 as i32)
                .max()
                .unwrap() as u32;
            let lh = app
                .outs
                .iter()
                .map(|o| o.logical_position.1 + o.size.1 as i32)
                .max()
                .unwrap() as u32;
            (
                vp_mgr.create_virtual_pointer(Some(&seat), &qh, NoEvents),
                (lw, lh),
                app.outs[i].logical_position,
            )
        };
        let mut errors = Vec::new();
        for &(tx, ty) in targets {
            vp.motion_absolute(
                now_ms(start),
                (tx + offset.0) as u32,
                (ty + offset.1) as u32,
                extent.0,
                extent.1,
            );
            vp.frame();
            let over = pump(
                &mut queue,
                &mut app,
                Duration::from_millis(300),
                |a| matches!(&a.focus, Some((s, _)) if *s == surface),
            );
            if !over {
                misses += 1;
                errors.push(serde_json::json!({"target": [tx, ty], "result": "pointer not over spike surface; no click sent"}));
                continue;
            }
            let before = app.presses.len();
            vp.button(now_ms(start), BTN_LEFT, wl_pointer::ButtonState::Pressed);
            vp.frame();
            vp.button(now_ms(start), BTN_LEFT, wl_pointer::ButtonState::Released);
            vp.frame();
            let got = pump(&mut queue, &mut app, Duration::from_millis(300), |a| {
                a.presses.len() > before
            });
            match app.presses.last().filter(|_| got) {
                Some((s, (x, y))) if *s == surface => {
                    let err = ((x - tx as f64).powi(2) + (y - ty as f64).powi(2)).sqrt();
                    worst = worst.max(err);
                    errors.push(
                        serde_json::json!({"target": [tx, ty], "landed": [x, y], "error_px": err}),
                    );
                }
                _ => {
                    misses += 1;
                    errors.push(
                        serde_json::json!({"target": [tx, ty], "result": "no press received"}),
                    );
                }
            }
        }
        vp.destroy();
        let max_here = errors
            .iter()
            .filter_map(|e| e["error_px"].as_f64())
            .fold(0.0, f64::max);
        println!(
            "   {}: {} targets, {} clicks landed, max error {:.2} px",
            app.outs[i].name,
            targets.len(),
            errors.iter().filter(|e| e["error_px"].is_number()).count(),
            max_here
        );
        report.push(serde_json::json!({"output": app.outs[i].name, "logical_size": [w, h], "clicks": errors}));
    }
    let go = misses == 0 && worst <= 2.0;
    println!(
        "   virtual pointer manager v{version}; worst error {worst:.2} px; misses {misses}  -> clicks {}",
        if go { "GO" } else { "NO-GO" }
    );
    if let Some(p) = json_path {
        let j = serde_json::json!({"virtual_pointer_version": version, "worst_px": worst, "misses": misses, "go": go, "outputs": report});
        std::fs::write(p, serde_json::to_string_pretty(&j).unwrap()).unwrap();
    }
}

fn active_class() -> String {
    let out = std::process::Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .ok();
    out.and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| v["class"].as_str().map(str::to_string))
        .unwrap_or_default()
}

fn upload(kb: &zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1, km: &Keymap) {
    let fd = unsafe { libc::memfd_create(c"handos-keymap".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0, "memfd_create failed");
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut file = std::fs::File::from(fd);
    file.write_all(km.text.as_bytes()).unwrap();
    file.write_all(&[0]).unwrap();
    kb.keymap(
        wl_keyboard::KeymapFormat::XkbV1.into(),
        file.as_fd(),
        km.text.len() as u32 + 1,
    );
}

fn cmd_type(args: &[String]) -> i32 {
    let method = flag(args, "--method").unwrap_or_else(|| "keymap".into());
    let text = flag(args, "--text").expect("--text TEXT");
    let enter = args.iter().any(|a| a == "--enter");
    let paste_keys = flag(args, "--paste-keys").unwrap_or_else(|| "ctrl-v".into());
    let delay = flag(args, "--delay-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let key_delay = flag(args, "--key-delay-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    std::thread::sleep(Duration::from_millis(delay));
    if let Some(want) = flag(args, "--expect-class") {
        let got = active_class();
        if !got.to_lowercase().contains(&want.to_lowercase()) {
            eprintln!("refusing to type: active window class is {got:?}, expected {want:?}");
            return 2;
        }
    }

    let conn = Connection::connect_to_env().expect("no Wayland display");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("registry");
    let qh = queue.handle();
    let mgr: zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1 = globals
        .bind(&qh, 1..=1, NoEvents)
        .expect("zwp_virtual_keyboard_manager_v1 not offered");
    let mut app = App::new(&globals, &qh);
    queue.roundtrip(&mut app).unwrap();
    let kb = mgr.create_virtual_keyboard(&app.seat(), &qh, NoEvents);
    let start = Instant::now();
    let tap = |code: u32| {
        kb.key(now_ms(start), code, wl_keyboard::KeyState::Pressed.into());
        kb.key(now_ms(start), code, wl_keyboard::KeyState::Released.into());
    };
    let t0 = Instant::now();
    let km = if method == "paste" {
        let status = std::process::Command::new("wl-copy")
            .arg("--")
            .arg(&text)
            .status()
            .expect("wl-copy");
        assert!(status.success(), "wl-copy failed");
        let km = Keymap::paste();
        upload(&kb, &km);
        queue.roundtrip(&mut app).unwrap();
        std::thread::sleep(Duration::from_millis(30)); // clients apply the new keymap
        km
    } else {
        let mut last = None;
        for (km, chunk) in Keymap::for_text(&text) {
            upload(&kb, &km);
            queue.roundtrip(&mut app).unwrap();
            std::thread::sleep(Duration::from_millis(30));
            for code in km.codes_for(&chunk) {
                tap(code);
                conn.flush().unwrap();
                std::thread::sleep(Duration::from_millis(key_delay));
            }
            last = Some(km);
        }
        last.expect("at least one chunk")
    };
    if method == "paste" {
        // Press real modifier keys and also set the modifier state, as a physical keyboard does.
        let shift = paste_keys == "ctrl-shift-v";
        let mods = if shift { CONTROL | SHIFT } else { CONTROL };
        let press = |code: u32, down: bool| {
            let st = if down {
                wl_keyboard::KeyState::Pressed
            } else {
                wl_keyboard::KeyState::Released
            };
            kb.key(now_ms(start), code, st.into());
        };
        press(km.code("Control_L").unwrap(), true);
        if shift {
            press(km.code("Shift_L").unwrap(), true);
        }
        kb.modifiers(mods, 0, 0, 0);
        conn.flush().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        tap(km.code("v").unwrap());
        conn.flush().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        if shift {
            press(km.code("Shift_L").unwrap(), false);
        }
        press(km.code("Control_L").unwrap(), false);
        kb.modifiers(0, 0, 0, 0);
    }
    if enter {
        queue.roundtrip(&mut app).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        tap(km.code("Return").unwrap());
    }
    queue.roundtrip(&mut app).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    kb.destroy();
    queue.roundtrip(&mut app).unwrap();
    println!(
        "typed {} chars by {method} in {:.0} ms",
        text.chars().count(),
        t0.elapsed().as_secs_f64() * 1000.0
    );
    0
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("clicks") => cmd_clicks(&args),
        Some("type") => std::process::exit(cmd_type(&args)),
        _ => {
            eprintln!(
                "usage: input-spike clicks [--cols N --rows N --json F] | type --method keymap|paste --text T [--enter] [--paste-keys ctrl-v|ctrl-shift-v] [--expect-class C] [--delay-ms N] [--key-delay-ms 2]"
            );
            std::process::exit(1);
        }
    }
}

// ------------------------------------------------------------------ Wayland plumbing

struct NoEvents;
impl Dispatch2<zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, App> for NoEvents {
    fn event(
        &self,
        _: &mut App,
        _: &zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
        _: zwlr_virtual_pointer_manager_v1::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}
impl Dispatch2<zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1, App> for NoEvents {
    fn event(
        &self,
        _: &mut App,
        _: &zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
        _: zwlr_virtual_pointer_v1::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}
impl Dispatch2<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, App> for NoEvents {
    fn event(
        &self,
        _: &mut App,
        _: &zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
        _: zwp_virtual_keyboard_manager_v1::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}
impl Dispatch2<zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1, App> for NoEvents {
    fn event(
        &self,
        _: &mut App,
        _: &zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
        _: zwp_virtual_keyboard_v1::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for e in events {
            match e.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.focus = Some((e.surface.clone(), e.position));
                }
                PointerEventKind::Leave { .. } => {
                    if matches!(&self.focus, Some((s, _)) if *s == e.surface) {
                        self.focus = None;
                    }
                }
                PointerEventKind::Press { .. } => {
                    self.presses.push((e.surface.clone(), e.position))
                }
                _ => {}
            }
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        cap: Capability,
    ) {
        if cap == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        cfg: LayerSurfaceConfigure,
        _: u32,
    ) {
        if let Some(o) = self.outs.iter_mut().find(|o| &o.layer == layer) {
            o.size = cfg.new_size;
            o.configured = true;
        }
    }
}

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
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
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

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_cover_grid_and_corners() {
        let t = targets_for(1920, 1080, 6, 4);
        assert_eq!(t.len(), 6 * 4 + 4);
        assert_eq!(t[0], (160, 135));
        assert!(t.contains(&(1916, 1076)));
        assert!(
            t.iter()
                .all(|&(x, y)| (0..1920).contains(&x) && (0..1080).contains(&y))
        );
    }
}
