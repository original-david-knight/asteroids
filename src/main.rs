use std::{error::Error, sync::Arc};

use asteroids::{
    audio::{AudioMsg, AudioMsgSender, AudioRuntime, AudioScaffold, VOICE_THRUST},
    renderer::{self, Renderer},
    runtime::{self, RuntimeConfig},
    tuning,
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    monitor::MonitorHandle,
    window::{Fullscreen, Window, WindowId},
};

fn main() -> Result<(), Box<dyn Error>> {
    let config = match RuntimeConfig::from_env_args() {
        Ok(config) => config,
        Err(message) if message.starts_with("Usage:") => {
            println!("{message}");
            return Ok(());
        }
        Err(message) => return Err(message.into()),
    };

    if !config.should_run_interactive() {
        return pollster::block_on(runtime::run_automated(&config)).map_err(Into::into);
    }

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
    audio: Option<AudioScaffold>,
    audio_sender: Option<AudioMsgSender>,
    audio_runtime: Option<AudioRuntime>,
    input: InputState,
    startup_error: Option<String>,
}

impl AsteroidsApp {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            audio: Some(AudioScaffold::new()),
            audio_sender: None,
            audio_runtime: None,
            input: InputState::default(),
            startup_error: None,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: impl Into<String>) {
        let error = error.into();
        eprintln!("{error}");
        self.startup_error = Some(error);
        event_loop.exit();
    }

    fn set_thrust_audio_gate(&mut self, active: bool) {
        let Some(sender) = self.audio_sender.as_mut() else {
            return;
        };
        let msg = if active {
            AudioMsg::Trigger(VOICE_THRUST)
        } else {
            AudioMsg::Release(VOICE_THRUST)
        };
        let _ = sender.try_push(msg);
    }
}

#[derive(Default)]
struct InputState {
    thrust_w: bool,
    thrust_up: bool,
}

#[derive(Clone, Copy)]
enum ThrustKey {
    W,
    Up,
}

impl InputState {
    fn thrust_active(&self) -> bool {
        self.thrust_w || self.thrust_up
    }

    fn update_thrust(&mut self, key: ThrustKey, pressed: bool) -> Option<bool> {
        let was_active = self.thrust_active();
        match key {
            ThrustKey::W => self.thrust_w = pressed,
            ThrustKey::Up => self.thrust_up = pressed,
        }
        let is_active = self.thrust_active();
        (was_active != is_active).then_some(is_active)
    }

    fn clear_thrust(&mut self) -> bool {
        let was_active = self.thrust_active();
        self.thrust_w = false;
        self.thrust_up = false;
        was_active
    }
}

fn thrust_key_for_event(event: &KeyEvent) -> Option<ThrustKey> {
    match &event.logical_key {
        Key::Named(NamedKey::ArrowUp) => Some(ThrustKey::Up),
        Key::Character(text) if text.eq_ignore_ascii_case("w") => Some(ThrustKey::W),
        _ => None,
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
                let Some(audio_scaffold) = self.audio.take() else {
                    self.fail(event_loop, "audio scaffold was already consumed");
                    return;
                };
                let voice_count = audio_scaffold.voices().len();
                let (audio_sender, receiver, voices) = audio_scaffold.into_parts();
                let audio_runtime = match AudioRuntime::start(receiver, voices, None) {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        self.fail(event_loop, error);
                        return;
                    }
                };
                let bloom = renderer.bloom_params();
                println!(
                    "display: {}; surface={}x{}, scale={:.3}, format={:?}, present_mode={:?}, phosphor={:?}, tau={:.0}ms, bloom_intensity={:.3}, bloom_threshold={:.3}",
                    renderer::display_server_note(),
                    renderer.size().width,
                    renderer.size().height,
                    window.scale_factor(),
                    renderer.surface_format(),
                    renderer.present_mode(),
                    renderer.phosphor_format(),
                    renderer.phosphor_tau_ms(),
                    bloom.intensity,
                    bloom.threshold,
                );
                println!("{}", audio_runtime.info().startup_summary());
                println!(
                    "audio messages: {voice_count} voices, channel capacity {} messages",
                    asteroids::audio::AUDIO_MSG_CAPACITY
                );
                self.audio_sender = Some(audio_sender);
                self.audio_runtime = Some(audio_runtime);
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
        let Some(active_window_id) = self.window.as_ref().map(|window| window.id()) else {
            return;
        };
        if active_window_id != window_id {
            return;
        }

        if let WindowEvent::KeyboardInput { event, .. } = &event
            && !event.repeat
            && let Some(key) = thrust_key_for_event(event)
        {
            let pressed = event.state == ElementState::Pressed;
            if let Some(active) = self.input.update_thrust(key, pressed) {
                self.set_thrust_audio_gate(active);
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(false) => {
                if self.input.clear_thrust() {
                    self.set_thrust_audio_gate(false);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => event_loop.exit(),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::F1),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let tau = renderer.reset_phosphor_tau_ms();
                    println!("debug phosphor tau reset: {tau:.0}ms");
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::F2),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let tau = renderer.adjust_phosphor_tau_ms(-tuning::PHOSPHOR_TAU_STEP_MS);
                    println!("debug phosphor tau: {tau:.0}ms");
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::F3),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let tau = renderer.adjust_phosphor_tau_ms(tuning::PHOSPHOR_TAU_STEP_MS);
                    println!("debug phosphor tau: {tau:.0}ms");
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::F4),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let bloom = renderer.reset_bloom_params();
                    println!(
                        "debug bloom reset: intensity={:.3}, threshold={:.3}",
                        bloom.intensity, bloom.threshold
                    );
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::F5),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let threshold = renderer.adjust_bloom_threshold(-tuning::BLOOM_THRESHOLD_STEP);
                    println!("debug bloom threshold: {threshold:.3}");
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::F6),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let threshold = renderer.adjust_bloom_threshold(tuning::BLOOM_THRESHOLD_STEP);
                    println!("debug bloom threshold: {threshold:.3}");
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::F7),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let intensity = renderer.adjust_bloom_intensity(-tuning::BLOOM_INTENSITY_STEP);
                    println!("debug bloom intensity: {intensity:.3}");
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        logical_key: Key::Named(NamedKey::F8),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                if let Some(renderer) = self.renderer.as_mut() {
                    let intensity = renderer.adjust_bloom_intensity(tuning::BLOOM_INTENSITY_STEP);
                    println!("debug bloom intensity: {intensity:.3}");
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let (Some(renderer), Some(window)) =
                    (self.renderer.as_mut(), self.window.as_ref())
                {
                    renderer.resize_for_window(window);
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let (Some(renderer), Some(window)) =
                    (self.renderer.as_mut(), self.window.as_ref())
                {
                    window.pre_present_notify();
                    if let Err(error) = renderer.render() {
                        self.fail(event_loop, error);
                        return;
                    }
                    window.request_redraw();
                }
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
