mod audio;
mod beam;
mod renderer;
mod tuning;

use std::{error::Error, sync::Arc};

use audio::AudioScaffold;
use renderer::Renderer;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    monitor::MonitorHandle,
    window::{Fullscreen, Window, WindowId},
};

fn main() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = AsteroidsApp::new();
    event_loop.run_app(&mut app)?;

    if let Some(error) = app.startup_error.take() {
        Err(error.into())
    } else {
        Ok(())
    }
}

struct AsteroidsApp {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    audio: AudioScaffold,
    startup_error: Option<String>,
}

impl AsteroidsApp {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            audio: AudioScaffold::new(),
            startup_error: None,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: impl Into<String>) {
        let error = error.into();
        eprintln!("{error}");
        self.startup_error = Some(error);
        event_loop.exit();
    }
}

impl ApplicationHandler for AsteroidsApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let monitor = preferred_fullscreen_monitor(event_loop);
        let target_size = monitor.as_ref().map(|monitor| monitor.size());
        let fullscreen = Some(Fullscreen::Borderless(monitor.clone()));
        let attributes = Window::default_attributes()
            .with_title("Asteroids")
            .with_decorations(false)
            .with_fullscreen(fullscreen);

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("failed to create fullscreen window: {error}"),
                );
                return;
            }
        };

        window.set_cursor_visible(false);

        match pollster::block_on(Renderer::new(Arc::clone(&window), target_size)) {
            Ok(renderer) => {
                println!(
                    "display: {}; surface={}x{}, scale={:.3}, format={:?}, present_mode={:?}",
                    renderer::display_server_note(),
                    renderer.size().width,
                    renderer.size().height,
                    window.scale_factor(),
                    renderer.surface_format(),
                    renderer.present_mode(),
                );
                println!(
                    "audio scaffold: {} voices, channel capacity {} messages, cpal stream not spawned",
                    self.audio.voices().len(),
                    audio::AUDIO_MSG_CAPACITY
                );
                self.renderer = Some(renderer);
                self.window = Some(window);
            }
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if window.id() != window_id {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize_for_window(window);
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Some(renderer) = self.renderer.as_mut() {
                    window.pre_present_notify();
                    if let Err(error) = renderer.render() {
                        self.fail(event_loop, error);
                        return;
                    }
                }
                window.request_redraw();
            }
            _ => {}
        }
    }
}

fn preferred_fullscreen_monitor(event_loop: &ActiveEventLoop) -> Option<MonitorHandle> {
    event_loop
        .available_monitors()
        .max_by_key(|monitor| {
            let size = monitor.size();
            u64::from(size.width) * u64::from(size.height)
        })
        .or_else(|| event_loop.primary_monitor())
}
