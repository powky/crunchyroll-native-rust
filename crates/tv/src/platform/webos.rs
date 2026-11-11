//! The TV: a full-screen Wayland surface with the window role from the core `wl_shell` (without a role webOS's
//! compositor never maps it) and webOS's own properties from `wl_webos_shell` (an app card, and the remote's keys),
//! an EGL window surface on it and an OpenGL ES 2 context. The
//! system's libwayland and libEGL are loaded at run time, so the build needs no TV libraries to link against.
//!
//! The surface is 1920x1080; the compositor scales it to the panel.

use std::ffi::c_void;
use std::os::fd::AsRawFd;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use khronos_egl as egl;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_compositor, wl_keyboard, wl_output, wl_region, wl_registry, wl_seat, wl_shell, wl_shell_surface, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};

use super::{App, Key};
use crate::gfx::{GlFlavor, Renderer};

#[allow(dead_code, non_camel_case_types, non_upper_case_globals, clippy::all)]
pub mod shell {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/webos-shell.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/webos-shell.xml");
}

#[allow(dead_code, non_camel_case_types, non_upper_case_globals, clippy::all)]
pub mod foreign {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/webos-foreign.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/webos-foreign.xml");
}

use foreign::wl_webos_exported::{self, WlWebosExported};
use foreign::wl_webos_foreign::WlWebosForeign;
use shell::wl_webos_shell::WlWebosShell;
use shell::wl_webos_shell_surface::{self, WlWebosShellSurface};

/// The UI's logical size; the surface is the screen's real size (3840x2160 on a 4K TV) and the renderer scales.
const WIDTH: i32 = 1920;
const HEIGHT: i32 = 1080;
/// A frame's time at 60 Hz: how long a frame the same as the last one waits for the remote before the next.
const FRAME: Duration = Duration::from_micros(16_667);
/// How long a screen at rest waits for the remote (or the worker's news, or a timer) before looking again.
const AT_REST: Duration = Duration::from_millis(250);

#[derive(Default)]
struct State {
    keys: Vec<Key>,
    keyboards: Vec<wl_keyboard::WlKeyboard>,
    /// the screen's current mode, from wl_output
    output: Option<(i32, i32)>,
    closed: bool,
}

pub fn run(mut app: impl App + 'static) -> Result<()> {
    // SAM starts native apps without the Wayland variables; webOS's compositor socket is /tmp/xdg/wayland-0
    let connection = match Connection::connect_to_env() {
        Ok(connection) => connection,
        Err(_) => {
            let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp/xdg".to_string());
            let name = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string());
            let socket = std::os::unix::net::UnixStream::connect(std::path::Path::new(&runtime).join(name))
                .context("connecting to the compositor socket")?;
            Connection::from_socket(socket).context("connecting to the compositor")?
        }
    };
    let (globals, mut queue) = registry_queue_init::<State>(&connection).context("reading the registry")?;
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=4, ()).context("wl_compositor")?;
    let core_shell: wl_shell::WlShell = globals.bind(&qh, 1..=1, ()).context("wl_shell")?;
    let shell: WlWebosShell = globals.bind(&qh, 1..=1, ()).context("wl_webos_shell")?;
    let foreign: Option<WlWebosForeign> = globals.bind(&qh, 1..=1, ()).ok();
    // the remote may sit on a seat of its own: take every one
    let mut seats = Vec::new();
    let mut outputs = Vec::new();
    for global in globals.contents().clone_list() {
        eprintln!("crunchy: global {} v{}", global.interface, global.version);
        if global.interface == "wl_seat" {
            seats.push(globals.registry().bind::<wl_seat::WlSeat, _, _>(global.name, global.version.min(5), &qh, ()));
        }
        if global.interface == "wl_output" {
            outputs.push(globals.registry().bind::<wl_output::WlOutput, _, _>(global.name, global.version.min(2), &qh, ()));
        }
    }

    let surface = compositor.create_surface(&qh, ());
    let window_role = core_shell.get_shell_surface(&surface, &qh, ());
    window_role.set_toplevel();
    let shell_surface = shell.get_shell_surface(&surface, &qh, ());
    shell_surface.set_property("appId".to_string(), super::app_id());
    shell_surface.set_property("_WEBOS_WINDOW_TYPE".to_string(), "_WEBOS_WINDOW_TYPE_CARD".to_string());
    // without it the system takes Back (what LG's SDL sets for SDL_WEBOS_ACCESS_POLICY_KEYS_BACK)
    shell_surface.set_property("_WEBOS_ACCESS_POLICY_KEYS_BACK".to_string(), "true".to_string());
    // Back and Exit come to the app; Home stays with the system
    shell_surface.set_key_mask(
        (shell::wl_webos_shell_surface::WebosKey::Default as u32) | (shell::wl_webos_shell_surface::WebosKey::Back as u32),
    );
    shell_surface.set_state(wl_webos_shell_surface::State::Fullscreen as u32);
    surface.commit();

    let mut state = State::default();
    queue.roundtrip(&mut state).context("first roundtrip")?;

    // EGL on the surface
    let egl = unsafe { egl::DynamicInstance::<egl::EGL1_4>::load_required_from_filename("libEGL.so") }
        .map_err(|e| anyhow!("loading EGL 1.4: {e}"))?;
    let display_ptr = connection.backend().display_ptr() as *mut c_void;
    let display = unsafe { egl.get_display(display_ptr) }.context("no EGL display")?;
    egl.initialize(display).context("initialising EGL")?;
    let attributes = [
        egl::RED_SIZE, 8,
        egl::GREEN_SIZE, 8,
        egl::BLUE_SIZE, 8,
        egl::ALPHA_SIZE, 8,
        egl::SURFACE_TYPE, egl::WINDOW_BIT,
        egl::RENDERABLE_TYPE, egl::OPENGL_ES2_BIT,
        egl::NONE,
    ];
    let config = egl.choose_first_config(display, &attributes)?.context("no EGL config")?;
    let context = egl.create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE])?;
    // the size the settings ask for (CRUNCHY_SIZE=WxH or /tmp/crunchy-size override it)
    let forced = forced_size();
    let (mut width, mut height) = forced.unwrap_or_else(super::surface_size_wanted);
    eprintln!("crunchy: output {:?}, drawing at {width}x{height}", state.output);
    let window = wayland_egl::WlEglSurface::new(surface.id(), width, height).context("wl_egl_window")?;
    let egl_surface = unsafe { egl.create_window_surface(display, config, window.ptr() as egl::NativeWindowType, None) }
        .context("EGL window surface")?;
    egl.make_current(display, Some(egl_surface), Some(egl_surface), Some(context))?;
    let _ = egl.swap_interval(display, 1);
    let gl = unsafe {
        glow::Context::from_loader_function(|name| {
            egl.get_proc_address(name).map(|f| f as *const c_void).unwrap_or(std::ptr::null())
        })
    };
    let mut renderer = unsafe { Renderer::new(gl, GlFlavor::Es2)? };
    renderer.resize(width as u32, height as u32);
    match background_uploads(&egl, display, config, context) {
        Ok(uploads) => renderer.uploads = Some(uploads),
        Err(error) => eprintln!("crunchy: pictures upload between frames: {error:#}"),
    }
    eprintln!("crunchy: EGL up, {width}x{height}");

    let mut frames: u64 = 0;
    // frame pacing over the last 600 frames: the average and the slowest
    let mut window_start = std::time::Instant::now();
    let mut last_frame = window_start;
    let mut worst = Duration::ZERO;
    // where the time goes: drawing (CPU, and the GL calls that wait on the GPU), the swap (the GPU finishing, the
    // compositor, the vblank), and how much was drawn
    let (mut drawing, mut swapping, mut fill, mut draws) = (Duration::ZERO, Duration::ZERO, 0.0f32, 0u32);
    let mut waiting = Duration::ZERO;
    let mut video: Option<WlWebosExported> = None;
    // frames the same as the one on screen aren't drawn or presented; without a swap to wait on, the loop waits for
    // the remote (or a frame's time) instead
    let mut presented = 0u32;
    // frames in a row the same as the one on screen: past half a second of them the screen is at rest, and is
    // looked at less often (a key still wakes it at once)
    let mut unchanged = 0u32;
    loop {
        let wait = match unchanged {
            0 => Duration::ZERO,
            1..=30 => FRAME,
            _ => AT_REST,
        };
        pump(&connection, &mut queue, &mut state, wait)?;
        match (&video, super::video_window_wanted(), &foreign) {
            (None, true, Some(foreign)) => {
                let exported = foreign.export_element(&surface, foreign::wl_webos_foreign::WebosExportedType::VideoObject as u32, &qh, ());
                // the whole screen: the source as the UI's 1920x1080, the destination in the surface's pixels
                let source = compositor.create_region(&qh, ());
                source.add(0, 0, WIDTH, HEIGHT);
                let destination = compositor.create_region(&qh, ());
                destination.add(0, 0, width, height);
                exported.set_exported_window(&source, &destination);
                source.destroy();
                destination.destroy();
                video = Some(exported);
                eprintln!("crunchy: video window asked for");
            }
            (Some(_), false, _) => {
                if let Some(exported) = video.take() {
                    exported.destroy();
                }
                super::set_video_window(None);
                eprintln!("crunchy: video window let go");
            }
            _ => {}
        }
        if state.closed {
            break;
        }
        // the settings changed the drawing size: the next buffer comes at the new one
        if forced.is_none() && super::surface_size_wanted() != (width, height) {
            (width, height) = super::surface_size_wanted();
            window.resize(width, height, 0, 0);
            renderer.resize(width as u32, height as u32);
            eprintln!("crunchy: drawing at {width}x{height}");
        }
        let keys = std::mem::take(&mut state.keys);
        let drawing_from = std::time::Instant::now();
        if !app.frame(&mut renderer, &keys) {
            break;
        }
        let swapping_from = std::time::Instant::now();
        drawing += swapping_from - drawing_from;
        fill += renderer.fill;
        draws += renderer.draw_calls;
        waiting += renderer.waited;
        if frames < 2 {
            eprintln!("crunchy: frame {frames} drawn, swapping");
        }
        unchanged = if renderer.presented || !keys.is_empty() { 0 } else { unchanged.saturating_add(1) };
        if renderer.presented {
            // blocks until the next vblank: the frame pacing
            egl.swap_buffers(display, egl_surface)?;
            presented += 1;
        }
        swapping += swapping_from.elapsed();
        if frames == 0 {
            // the compositor ignores the state of a surface that has no buffer yet: set it again now it has one
            shell_surface.set_state(wl_webos_shell_surface::State::Fullscreen as u32);
            surface.commit();
            connection.flush()?;
        }
        frames += 1;
        let now = std::time::Instant::now();
        worst = worst.max(now - last_frame);
        last_frame = now;
        if frames < 3 {
            eprintln!("crunchy: {frames} frames");
        } else if frames % 600 == 0 {
            let average = (now - window_start).as_secs_f32() * 1000.0 / 600.0;
            let per_frame = |time: Duration| time.as_secs_f32() * 1000.0 / 600.0;
            eprintln!(
                "crunchy: {frames} frames ({presented} drawn), {average:.1} ms average, {:.1} ms worst; drawing {:.1} ms \
                 (waiting {:.1}), swap {:.1} ms, {:.2} screens filled, {} draws a frame",
                worst.as_secs_f32() * 1000.0,
                per_frame(drawing),
                per_frame(waiting),
                per_frame(swapping),
                fill / 600.0 / (WIDTH * HEIGHT) as f32,
                draws / 600,
            );
            window_start = now;
            worst = Duration::ZERO;
            presented = 0;
            (drawing, swapping, fill, draws) = (Duration::ZERO, Duration::ZERO, 0.0, 0);
            waiting = Duration::ZERO;
        }
    }
    drop(shell_surface);
    drop(window_role);
    drop(seats);
    drop(outputs);
    Ok(())
}

/// A size to try on the TV in place of the settings', where SAM passes no environment: a file in /tmp.
///
/// The UI plane is 1920x1080 whatever the surface (`/proc/lg/fbdev/osdinfo`: input and output windows 1920x1080,
/// with the web app in front too): the compositor scales a bigger surface down to it, and the TV scales that up to
/// the panel. Drawing bigger costs GPU time and gains nothing on this TV.
fn forced_size() -> Option<(i32, i32)> {
    let wanted = std::env::var("CRUNCHY_SIZE").ok().or_else(|| std::fs::read_to_string("/tmp/crunchy-size").ok())?;
    let (w, h) = wanted.trim().split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// A thread that uploads pictures through a context sharing textures with the drawing one (current without a
/// surface: EGL_KHR_surfaceless_context).
fn background_uploads(
    egl: &egl::DynamicInstance<egl::EGL1_4>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
) -> Result<crate::gfx::Uploads> {
    let extensions = egl.query_string(Some(display), egl::EXTENSIONS)?.to_string_lossy().into_owned();
    if !extensions.contains("EGL_KHR_surfaceless_context") {
        anyhow::bail!("no EGL_KHR_surfaceless_context");
    }
    let shared = egl.create_context(display, config, Some(context), &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE])?;
    // EGL's handles aren't Send: across as numbers, the same display and context on the other side
    let (display, shared) = (display.as_ptr() as usize, shared.as_ptr() as usize);
    crate::gfx::Uploads::start(move || {
        let egl = unsafe { egl::DynamicInstance::<egl::EGL1_4>::load_required_from_filename("libEGL.so") }
            .map_err(|e| anyhow!("loading EGL 1.4: {e}"))?;
        let display = unsafe { egl::Display::from_ptr(display as egl::EGLDisplay) };
        let shared = unsafe { egl::Context::from_ptr(shared as egl::EGLContext) };
        egl.make_current(display, None, None, Some(shared))?;
        Ok(unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name).map(|f| f as *const c_void).unwrap_or(std::ptr::null())
            })
        })
    })
}

/// Handles whatever the compositor sent, waiting up to `wait` for something to come.
fn pump(connection: &Connection, queue: &mut wayland_client::EventQueue<State>, state: &mut State, wait: Duration) -> Result<()> {
    queue.dispatch_pending(state)?;
    connection.flush()?;
    if let Some(guard) = queue.prepare_read() {
        let fd = guard.connection_fd().as_raw_fd();
        if readable(fd, wait) {
            guard.read()?;
        }
    }
    queue.dispatch_pending(state)?;
    Ok(())
}

fn readable(fd: i32, timeout: Duration) -> bool {
    use std::ffi::{c_int, c_short, c_ulong};
    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: c_short,
        revents: c_short,
    }
    unsafe extern "C" {
        // nfds_t is an unsigned long: 32 bits on the TV
        fn poll(fds: *mut PollFd, count: c_ulong, timeout: c_int) -> c_int;
    }
    let mut fds = [PollFd { fd, events: 1, revents: 0 }];
    let ready = unsafe { poll(fds.as_mut_ptr(), 1, timeout.as_millis() as i32) };
    ready > 0 && fds[0].revents & 1 != 0
}

/// Linux input codes of the LG remote's keys (Wayland reports evdev codes).
fn map_key(code: u32) -> Key {
    match code {
        103 => Key::Up,
        108 => Key::Down,
        105 => Key::Left,
        106 => Key::Right,
        28 | 96 | 352 => Key::Ok,
        1 | 14 | 158 | 412 => Key::Back,
        other => Key::Other(other),
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlWebosForeign, ()> for State {
    fn event(_: &mut Self, _: &WlWebosForeign, _: foreign::wl_webos_foreign::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlWebosExported, ()> for State {
    fn event(_: &mut Self, _: &WlWebosExported, event: wl_webos_exported::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let wl_webos_exported::Event::WindowIdAssigned { window_id, exported_type } = event;
        eprintln!("crunchy: video window {window_id} (type {exported_type})");
        super::set_video_window(Some(window_id));
    }
}

impl Dispatch<wl_region::WlRegion, ()> for State {
    fn event(_: &mut Self, _: &wl_region::WlRegion, _: wl_region::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_compositor::WlCompositor, ()> for State {
    fn event(_: &mut Self, _: &wl_compositor::WlCompositor, _: wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn event(_: &mut Self, _: &wl_surface::WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_shell::WlShell, ()> for State {
    fn event(_: &mut Self, _: &wl_shell::WlShell, _: wl_shell::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_shell_surface::WlShellSurface, ()> for State {
    fn event(_: &mut Self, role: &wl_shell_surface::WlShellSurface, event: wl_shell_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // the compositor checks the app is alive
        if let wl_shell_surface::Event::Ping { serial } = event {
            role.pong(serial);
        }
    }
}

impl Dispatch<WlWebosShell, ()> for State {
    fn event(_: &mut Self, _: &WlWebosShell, _: shell::wl_webos_shell::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlWebosShellSurface, ()> for State {
    fn event(state: &mut Self, _: &WlWebosShellSurface, event: wl_webos_shell_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_webos_shell_surface::Event::Close => state.closed = true,
            other => eprintln!("crunchy: shell event {other:?}"),
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(state: &mut Self, _: &wl_output::WlOutput, event: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_output::Event::Mode { flags: WEnum::Value(flags), width, height, refresh } => {
                eprintln!("crunchy: output mode {width}x{height} @{refresh} {flags:?}");
                if flags.contains(wl_output::Mode::Current) {
                    state.output = Some((width, height));
                }
            }
            wl_output::Event::Scale { factor } => eprintln!("crunchy: output scale {factor}"),
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_seat::Event::Capabilities { capabilities: WEnum::Value(capabilities) } => {
                eprintln!("crunchy: seat {:?} has {capabilities:?}", seat.id());
                if capabilities.contains(wl_seat::Capability::Keyboard) {
                    state.keyboards.push(seat.get_keyboard(qh, ()));
                }
            }
            wl_seat::Event::Name { name } => eprintln!("crunchy: seat {:?} is {name}", seat.id()),
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(state: &mut Self, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Key { key, state: WEnum::Value(wl_keyboard::KeyState::Pressed), .. } => {
                let mapped = map_key(key);
                eprintln!("crunchy: key {key} -> {mapped:?}");
                state.keys.push(mapped);
            }
            wl_keyboard::Event::Enter { .. } => eprintln!("crunchy: keyboard focus in"),
            wl_keyboard::Event::Leave { .. } => eprintln!("crunchy: keyboard focus out"),
            _ => {}
        }
    }
}
