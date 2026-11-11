//! The simulator: a 960x540 window (1920x1080 pixels on a Retina screen) with desktop OpenGL, arrow keys, Enter
//! and Escape/Backspace for Back. `CRUNCHY_SHOT=<file.png>` saves frame `CRUNCHY_SHOT_FRAME` (default 90) and quits.
//! `CRUNCHY_KEYS=ok,right,w60,down` presses keys on its own, one every 12 frames from frame 30 (`wN` waits N frames).

use std::collections::VecDeque;
use std::num::NonZeroU32;

use anyhow::{Context, Result, anyhow};
use glow::HasContext;
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, GlProfile, NotCurrentGlContext, PossiblyCurrentContext, Version};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, NamedKey};
use winit::window::{Window, WindowId};

use super::{App, Key};
use crate::gfx::{GlFlavor, Renderer};

struct Running {
    window: Window,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    renderer: Renderer,
}

struct Simulator<A: App> {
    app: A,
    running: Option<Running>,
    keys: Vec<Key>,
    frame: u32,
    shot: Option<(String, u32)>,
    /// keys to press on their own, and the frame the next one goes at
    script: VecDeque<String>,
    next_key: u32,
    error: Option<anyhow::Error>,
}

pub fn run(app: impl App + 'static) -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let shot = std::env::var("CRUNCHY_SHOT").ok().map(|path| {
        let frame = std::env::var("CRUNCHY_SHOT_FRAME").ok().and_then(|f| f.parse().ok()).unwrap_or(90);
        (path, frame)
    });
    let script = std::env::var("CRUNCHY_KEYS")
        .map(|keys| keys.split(',').map(|key| key.trim().to_lowercase()).filter(|key| !key.is_empty()).collect())
        .unwrap_or_default();
    let mut simulator =
        Simulator { app, running: None, keys: Vec::new(), frame: 0, shot, script, next_key: 30, error: None };
    event_loop.run_app(&mut simulator)?;
    match simulator.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

impl<A: App> Simulator<A> {
    fn create(&mut self, event_loop: &ActiveEventLoop) -> Result<Running> {
        let attributes = Window::default_attributes()
            .with_title("Crunchy (simulator)")
            .with_inner_size(LogicalSize::new(960.0, 540.0))
            .with_resizable(false);
        let (window, config) = DisplayBuilder::new()
            .with_window_attributes(Some(attributes))
            .build(event_loop, ConfigTemplateBuilder::new().with_alpha_size(8), |mut configs| {
                configs.next().expect("a GL config")
            })
            .map_err(|e| anyhow!("{e}"))?;
        let window = window.context("no window")?;
        let raw = window.window_handle().ok().map(|handle| handle.as_raw());
        let display = config.display();
        let attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 2))))
            .with_profile(GlProfile::Core)
            .build(raw);
        let context = unsafe { display.create_context(&config, &attributes)? };
        let surface_attributes = window.build_surface_attributes(Default::default())?;
        let surface = unsafe { display.create_window_surface(&config, &surface_attributes)? };
        let context = context.make_current(&surface)?;
        let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()));
        let gl = unsafe { glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name)) };
        let mut renderer = unsafe { Renderer::new(gl, GlFlavor::DesktopCore)? };
        // screenshots read the frame back: there must always be one
        renderer.always_draw = true;
        Ok(Running { window, surface, context, renderer })
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(running) = self.running.as_mut() else { return };
        let size = running.window.inner_size();
        while self.frame >= self.next_key
            && let Some(token) = self.script.pop_front()
        {
            self.next_key = self.frame + 12;
            let key = match token.as_str() {
                "up" => Key::Up,
                "down" => Key::Down,
                "left" => Key::Left,
                "right" => Key::Right,
                "ok" => Key::Ok,
                "back" => Key::Back,
                wait => {
                    self.next_key = self.frame + wait.trim_start_matches('w').parse::<u32>().unwrap_or(0);
                    continue;
                }
            };
            self.keys.push(key);
        }
        let keys = std::mem::take(&mut self.keys);
        running.renderer.resize(size.width, size.height);
        let keep = self.app.frame(&mut running.renderer, &keys);
        self.frame += 1;
        if let Some((path, frame)) = &self.shot
            && self.frame == *frame
        {
            if let Err(error) = save_shot(&running.renderer.gl, size.width, size.height, path) {
                eprintln!("screenshot failed: {error:#}");
            }
            event_loop.exit();
        }
        if let Err(error) = running.surface.swap_buffers(&running.context) {
            self.error = Some(error.into());
            event_loop.exit();
        }
        if !keep {
            event_loop.exit();
        }
        running.window.request_redraw();
    }
}

impl<A: App> ApplicationHandler for Simulator<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_none() {
            match self.create(event_loop) {
                Ok(running) => {
                    running.window.request_redraw();
                    self.running = Some(running);
                }
                Err(error) => {
                    self.error = Some(error);
                    event_loop.exit();
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let key = match event.logical_key {
                    WinitKey::Named(NamedKey::ArrowUp) => Key::Up,
                    WinitKey::Named(NamedKey::ArrowDown) => Key::Down,
                    WinitKey::Named(NamedKey::ArrowLeft) => Key::Left,
                    WinitKey::Named(NamedKey::ArrowRight) => Key::Right,
                    WinitKey::Named(NamedKey::Enter) => Key::Ok,
                    WinitKey::Named(NamedKey::Escape | NamedKey::Backspace) => Key::Back,
                    _ => return,
                };
                self.keys.push(key);
            }
            _ => {}
        }
    }
}

fn save_shot(gl: &glow::Context, width: u32, height: u32, path: &str) -> Result<()> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    unsafe {
        gl.read_pixels(
            0,
            0,
            width as i32,
            height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
    }
    // GL rows go bottom-up
    let row = (width * 4) as usize;
    let flipped: Vec<u8> = pixels.chunks(row).rev().flatten().copied().collect();
    image::save_buffer(path, &flipped, width, height, image::ColorType::Rgba8)?;
    Ok(())
}
