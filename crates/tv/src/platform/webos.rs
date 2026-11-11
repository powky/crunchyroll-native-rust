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
use wayland_client::protocol::{wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shell, wl_shell_surface, wl_surface};
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

use shell::wl_webos_shell::WlWebosShell;
use shell::wl_webos_shell_surface::{self, WlWebosShellSurface};

const WIDTH: i32 = 1920;
const HEIGHT: i32 = 1080;
const APP_ID: &str = "com.ian.crunchy";

#[derive(Default)]
struct State {
    keys: Vec<Key>,
    keyboards: Vec<wl_keyboard::WlKeyboard>,
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
    // the remote may sit on a seat of its own: take every one
    let mut seats = Vec::new();
    for global in globals.contents().clone_list() {
        eprintln!("crunchy: global {} v{}", global.interface, global.version);
        if global.interface == "wl_seat" {
            seats.push(globals.registry().bind::<wl_seat::WlSeat, _, _>(global.name, global.version.min(5), &qh, ()));
        }
    }

    let surface = compositor.create_surface(&qh, ());
    let window_role = core_shell.get_shell_surface(&surface, &qh, ());
    window_role.set_toplevel();
    let shell_surface = shell.get_shell_surface(&surface, &qh, ());
    let app_id = std::env::var("APPID").unwrap_or_else(|_| APP_ID.to_string());
    shell_surface.set_property("appId".to_string(), app_id);
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
    let window = wayland_egl::WlEglSurface::new(surface.id(), WIDTH, HEIGHT).context("wl_egl_window")?;
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
    renderer.resize(WIDTH as u32, HEIGHT as u32);
    eprintln!("crunchy: EGL up, {WIDTH}x{HEIGHT}");

    let mut frames: u64 = 0;
    loop {
        pump(&connection, &mut queue, &mut state)?;
        if state.closed {
            break;
        }
        let keys = std::mem::take(&mut state.keys);
        if !app.frame(&mut renderer, &keys) {
            break;
        }
        if frames < 2 {
            eprintln!("crunchy: frame {frames} drawn, swapping");
        }
        // blocks until the next vblank: the frame pacing
        egl.swap_buffers(display, egl_surface)?;
        if frames == 0 {
            // the compositor ignores the state of a surface that has no buffer yet: set it again now it has one
            shell_surface.set_state(wl_webos_shell_surface::State::Fullscreen as u32);
            surface.commit();
            connection.flush()?;
        }
        frames += 1;
        if frames < 3 || frames % 600 == 0 {
            eprintln!("crunchy: {frames} frames");
        }
    }
    drop(shell_surface);
    drop(window_role);
    drop(seats);
    Ok(())
}

/// Handles whatever the compositor sent since the last frame, without waiting.
fn pump(connection: &Connection, queue: &mut wayland_client::EventQueue<State>, state: &mut State) -> Result<()> {
    queue.dispatch_pending(state)?;
    connection.flush()?;
    if let Some(guard) = queue.prepare_read() {
        let fd = guard.connection_fd().as_raw_fd();
        if readable(fd, Duration::ZERO) {
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
