use std::{
    env,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use winit::dpi::PhysicalSize;

use crate::{
    audio,
    game::{self, AsteroidSize, ControlState, GameEvent, GameEventKind, GameLoop},
    renderer::{FrameParams, HeadlessRenderer, Scenario},
    rng::{SeededRng, rng_for_seed},
    tuning, verify,
};

const DEFAULT_FIXED_DT_SECONDS: f32 = 1.0 / 144.0;
pub const FRAME_METADATA_FILENAME: &str = "frame_metadata.jsonl";

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub headless: bool,
    pub screenshot: Option<PathBuf>,
    pub capture_frames: Option<usize>,
    pub frames_out: Option<PathBuf>,
    pub audio_capture_secs: Option<f64>,
    pub wav_out: Option<PathBuf>,
    pub seed: Option<u64>,
    pub fixed_dt: Option<f32>,
    pub simulate_secs: Option<f64>,
    pub scenario: Scenario,
    pub xrun_log: Option<PathBuf>,
    pub frame_time_log: Option<PathBuf>,
    pub state_log: Option<PathBuf>,
    pub bloom_intensity: f32,
    pub bloom_threshold: f32,
    saw_flag: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            headless: false,
            screenshot: None,
            capture_frames: None,
            frames_out: None,
            audio_capture_secs: None,
            wav_out: None,
            seed: None,
            fixed_dt: None,
            simulate_secs: None,
            scenario: Scenario::Demo,
            xrun_log: None,
            frame_time_log: None,
            state_log: None,
            bloom_intensity: tuning::BLOOM_INTENSITY_DEFAULT,
            bloom_threshold: tuning::BLOOM_THRESHOLD_DEFAULT,
            saw_flag: false,
        }
    }
}

impl RuntimeConfig {
    pub fn from_env_args() -> Result<Self, String> {
        Self::from_args(env::args().skip(1))
    }

    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut config = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            config.saw_flag = true;
            match arg.as_str() {
                "--help" | "-h" => return Err(runtime_usage()),
                "--headless" => config.headless = true,
                "--screenshot" => config.screenshot = Some(next_path(&mut args, "--screenshot")?),
                "--capture-frames" => {
                    config.capture_frames = Some(next_value(&mut args, "--capture-frames")?)
                }
                "--frames-out" => config.frames_out = Some(next_path(&mut args, "--frames-out")?),
                "--audio-capture" => {
                    config.audio_capture_secs = Some(next_value(&mut args, "--audio-capture")?)
                }
                "--wav-out" => config.wav_out = Some(next_path(&mut args, "--wav-out")?),
                "--seed" => config.seed = Some(next_value(&mut args, "--seed")?),
                "--fixed-dt" => config.fixed_dt = Some(next_value(&mut args, "--fixed-dt")?),
                "--simulate-secs" => {
                    config.simulate_secs = Some(next_value(&mut args, "--simulate-secs")?)
                }
                "--scenario" => {
                    let name = next_string(&mut args, "--scenario")?;
                    config.scenario = Scenario::parse(&name)
                        .ok_or_else(|| format!("unknown scenario '{name}'"))?;
                }
                "--xrun-log" => config.xrun_log = Some(next_path(&mut args, "--xrun-log")?),
                "--frame-time-log" => {
                    config.frame_time_log = Some(next_path(&mut args, "--frame-time-log")?)
                }
                "--state-log" => config.state_log = Some(next_path(&mut args, "--state-log")?),
                "--bloom-intensity" => {
                    config.bloom_intensity = next_value(&mut args, "--bloom-intensity")?
                }
                "--bloom-threshold" => {
                    config.bloom_threshold = next_value(&mut args, "--bloom-threshold")?
                }
                _ => return Err(format!("unknown argument '{arg}'\n\n{}", runtime_usage())),
            }
        }
        config.validate()?;
        Ok(config)
    }

    pub fn should_run_interactive(&self) -> bool {
        !self.saw_flag
    }

    fn validate(&self) -> Result<(), String> {
        if self.capture_frames.is_some() && self.frames_out.is_none() {
            return Err("--capture-frames requires --frames-out <dir>".to_string());
        }
        if self.audio_capture_secs.is_some() && self.wav_out.is_none() {
            return Err("--audio-capture requires --wav-out <path>".to_string());
        }
        if matches!(self.capture_frames, Some(0)) {
            return Err("--capture-frames must be greater than zero".to_string());
        }
        if matches!(self.fixed_dt, Some(dt) if !dt.is_finite() || dt <= 0.0) {
            return Err("--fixed-dt must be a finite positive number".to_string());
        }
        if matches!(self.simulate_secs, Some(secs) if !secs.is_finite() || secs < 0.0) {
            return Err("--simulate-secs must be finite and non-negative".to_string());
        }
        if matches!(self.audio_capture_secs, Some(secs) if !secs.is_finite() || secs < 0.0) {
            return Err("--audio-capture must be finite and non-negative".to_string());
        }
        if !self.bloom_intensity.is_finite() || self.bloom_intensity < tuning::BLOOM_INTENSITY_MIN {
            return Err("--bloom-intensity must be a finite non-negative number".to_string());
        }
        if !self.bloom_threshold.is_finite() || self.bloom_threshold < tuning::BLOOM_THRESHOLD_MIN {
            return Err("--bloom-threshold must be a finite non-negative number".to_string());
        }
        Ok(())
    }
}

pub async fn run_automated(config: &RuntimeConfig) -> Result<(), String> {
    let fixed_dt = config.fixed_dt.unwrap_or(DEFAULT_FIXED_DT_SECONDS);
    let render_size = headless_render_size();
    let mut renderer = HeadlessRenderer::new(render_size).await?;
    renderer.set_bloom_params(config.bloom_intensity, config.bloom_threshold);
    eprintln!(
        "headless render: {}x{}, phosphor={:?}, scenario={}, seed={}, bloom_intensity={:.3}, bloom_threshold={:.3}",
        renderer.size().width,
        renderer.size().height,
        renderer.phosphor_format(),
        config.scenario.name(),
        config.seed.unwrap_or(0),
        config.bloom_intensity,
        config.bloom_threshold,
    );

    let mut audio_writer = start_automated_audio_capture(config)?;

    let mut frame_time_log = optional_writer(config.frame_time_log.as_deref())?;
    let mut state_log = optional_writer(config.state_log.as_deref())?;
    let mut tick_state = TickState::new(config.seed);
    let mut game_loop = game_loop_for_scenario(config.scenario, config.seed);

    if let Some(writer) = state_log.as_mut() {
        write_state_event(writer, &mut tick_state, config, "scenario-start")?;
    }

    let simulate_frames = config
        .simulate_secs
        .map(|secs| frames_for_duration(secs, fixed_dt))
        .unwrap_or(0);
    for _ in 0..simulate_frames {
        render_tick(
            &mut renderer,
            config,
            fixed_dt,
            &mut tick_state,
            &mut game_loop,
            TickIo {
                audio_sender: audio_writer.as_mut().map(|writer| &mut writer.sender),
                frame_time_log: frame_time_log.as_mut(),
                state_log: state_log.as_mut(),
            },
        )?;
    }

    if let (Some(frame_count), Some(frames_out)) = (config.capture_frames, &config.frames_out) {
        fs::create_dir_all(frames_out)
            .map_err(|error| format!("failed to create {}: {error}", frames_out.display()))?;
        let frame_metadata_path = frames_out.join(FRAME_METADATA_FILENAME);
        let mut frame_metadata = optional_writer(Some(frame_metadata_path.as_path()))?
            .ok_or_else(|| "failed to open frame metadata writer".to_string())?;
        let frame_digits = capture_frame_digits(frame_count);
        for frame_index in 0..frame_count {
            render_tick(
                &mut renderer,
                config,
                fixed_dt,
                &mut tick_state,
                &mut game_loop,
                TickIo {
                    audio_sender: audio_writer.as_mut().map(|writer| &mut writer.sender),
                    frame_time_log: frame_time_log.as_mut(),
                    state_log: state_log.as_mut(),
                },
            )?;
            let rgba = renderer.capture_rgba8()?;
            let path = frames_out.join(format!("frame_{frame_index:0frame_digits$}.png"));
            verify::save_png(&path, renderer.size().width, renderer.size().height, &rgba)?;
            write_frame_metadata(&mut frame_metadata, frame_index, config, &game_loop)?;
        }
        frame_metadata
            .flush()
            .map_err(|error| format!("failed to flush frame metadata: {error}"))?;
    }

    if let Some(path) = &config.screenshot {
        if simulate_frames == 0 && config.capture_frames.is_none() {
            render_tick(
                &mut renderer,
                config,
                fixed_dt,
                &mut tick_state,
                &mut game_loop,
                TickIo {
                    audio_sender: audio_writer.as_mut().map(|writer| &mut writer.sender),
                    frame_time_log: frame_time_log.as_mut(),
                    state_log: state_log.as_mut(),
                },
            )?;
        }
        let rgba = renderer.capture_rgba8()?;
        verify::save_png(path, renderer.size().width, renderer.size().height, &rgba)?;
    }

    if let Some(audio_writer) = audio_writer.as_mut() {
        audio_writer.drive_scripted_audio(config, state_log.as_mut())?;
    }

    if simulate_frames == 0
        && config.capture_frames.is_none()
        && config.screenshot.is_none()
        && config.audio_capture_secs.is_none()
    {
        render_tick(
            &mut renderer,
            config,
            fixed_dt,
            &mut tick_state,
            &mut game_loop,
            TickIo {
                audio_sender: audio_writer.as_mut().map(|writer| &mut writer.sender),
                frame_time_log: frame_time_log.as_mut(),
                state_log: state_log.as_mut(),
            },
        )?;
    }

    if let Some(writer) = frame_time_log.as_mut() {
        writer
            .flush()
            .map_err(|error| format!("failed to flush frame-time log: {error}"))?;
    }
    if let Some(writer) = state_log.as_mut() {
        writer
            .flush()
            .map_err(|error| format!("failed to flush state log: {error}"))?;
    }
    if let Some(mut audio_writer) = audio_writer {
        audio_writer.release_due_thrust_gate()?;
        let AutomatedAudioCapture {
            path,
            handle,
            runtime,
            capture_xruns,
            ..
        } = audio_writer;
        handle
            .join()
            .map_err(|_| format!("captured WAV writer panicked for {}", path.display()))?
            .map_err(|error| format!("failed to write captured WAV {}: {error}", path.display()))?;
        let xrun_count = runtime.stream_error_count() + capture_xruns.load(Ordering::Relaxed);
        if xrun_count > 0
            && let Some(error) = runtime.first_stream_error()
        {
            eprintln!("audio stream first error: {error}");
        }
        if let Some(path) = &config.xrun_log {
            audio::write_xrun_log(path, xrun_count)
                .map_err(|error| format!("failed to write xrun log {}: {error}", path.display()))?;
        }
    }
    Ok(())
}

struct AutomatedAudioCapture {
    path: PathBuf,
    handle: thread::JoinHandle<std::io::Result<()>>,
    runtime: audio::AudioRuntime,
    sender: audio::AudioMsgSender,
    capture_xruns: Arc<AtomicU64>,
    thrust_release_deadline: Option<Instant>,
}

impl AutomatedAudioCapture {
    fn drive_scripted_audio(
        &mut self,
        config: &RuntimeConfig,
        state_log: Option<&mut BufWriter<File>>,
    ) -> Result<(), String> {
        if !config.scenario.uses_scripted_audio() {
            return Ok(());
        }
        let duration = Duration::from_secs_f64(config.audio_capture_secs.unwrap_or(0.0));
        let start = Instant::now();
        match config.scenario {
            Scenario::Fire3 => self.drive_fire_3(start, duration),
            Scenario::ExplosionStorm => self.drive_explosion_storm(start, duration),
            Scenario::HeartbeatCurve => {
                self.drive_heartbeat_curve(config, start, duration, state_log)
            }
            Scenario::UfoLarge | Scenario::UfoSmall => {
                sleep_until(start + duration);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn drive_fire_3(&mut self, start: Instant, duration: Duration) -> Result<(), String> {
        for offset in [
            Duration::from_millis(60),
            Duration::from_millis(360),
            Duration::from_millis(660),
        ] {
            if offset <= duration {
                sleep_until(start + offset);
                enqueue_audio_msg(
                    &mut self.sender,
                    audio::AudioMsg::Trigger(audio::VOICE_FIRE),
                    "fire trigger",
                )?;
            }
        }
        sleep_until(start + duration);
        Ok(())
    }

    fn drive_explosion_storm(&mut self, start: Instant, duration: Duration) -> Result<(), String> {
        let burst_interval = Duration::from_millis(45);
        let mut burst = 0_u32;
        loop {
            let target = start + burst_interval.saturating_mul(burst);
            if target >= start + duration {
                break;
            }
            sleep_until(target);
            for index in 0..9 {
                let variant = ((burst + index) % 3) as u16;
                enqueue_audio_msg(
                    &mut self.sender,
                    audio::AudioMsg::TriggerVariant(audio::VOICE_EXPLOSION, variant),
                    "explosion trigger",
                )?;
            }
            burst += 1;
        }
        sleep_until(start + duration);
        Ok(())
    }

    fn drive_heartbeat_curve(
        &mut self,
        config: &RuntimeConfig,
        start: Instant,
        duration: Duration,
        mut state_log: Option<&mut BufWriter<File>>,
    ) -> Result<(), String> {
        let tick_interval = Duration::from_secs_f64(1.0 / 60.0);
        let mut tick = 0_u64;
        let mut next_tick = start;
        while next_tick < start + duration {
            sleep_until(next_tick);
            let elapsed = next_tick.duration_since(start).as_secs_f32();
            let asteroid_count = heartbeat_curve_asteroid_count(elapsed);
            enqueue_audio_msg(
                &mut self.sender,
                audio::AudioMsg::GameState(audio::GameSnapshot::with_game_over(
                    asteroid_count,
                    true,
                    0,
                    false,
                )),
                "heartbeat game state",
            )?;
            if let Some(writer) = state_log.as_deref_mut() {
                write_audio_state_tick_event(writer, config, tick, elapsed, asteroid_count)?;
            }
            tick += 1;
            next_tick += tick_interval;
        }
        Ok(())
    }

    fn release_due_thrust_gate(&mut self) -> Result<(), String> {
        let Some(deadline) = self.thrust_release_deadline.take() else {
            return Ok(());
        };
        let now = Instant::now();
        if deadline > now {
            thread::sleep(deadline - now);
        }
        enqueue_audio_msg(
            &mut self.sender,
            audio::AudioMsg::Release(audio::VOICE_THRUST),
            "thrust release",
        )
    }
}

fn start_automated_audio_capture(
    config: &RuntimeConfig,
) -> Result<Option<AutomatedAudioCapture>, String> {
    let (Some(secs), Some(path)) = (config.audio_capture_secs, &config.wav_out) else {
        if let Some(path) = &config.xrun_log {
            create_empty_file(path)?;
        }
        return Ok(None);
    };

    let (capture_producer, capture_consumer, capture_xruns) = audio::audio_capture_channel();
    let scaffold = audio::AudioScaffold::new();
    let (mut sender, receiver, voices) = scaffold.into_parts();
    let runtime = audio::AudioRuntime::start(receiver, voices, Some(capture_producer))?;
    eprintln!("{}", runtime.info().startup_summary());

    let thrust_release_deadline = match config.scenario {
        Scenario::Thrust1s | Scenario::ShipSpinningWithThrust => {
            enqueue_audio_msg(
                &mut sender,
                audio::AudioMsg::Trigger(audio::VOICE_THRUST),
                "thrust trigger",
            )?;
            (config.scenario == Scenario::Thrust1s).then(|| Instant::now() + Duration::from_secs(1))
        }
        _ => None,
    };

    match config.scenario {
        Scenario::UfoLarge | Scenario::UfoSmall => {
            let variant = if config.scenario == Scenario::UfoSmall {
                1.0
            } else {
                0.0
            };
            if config.simulate_secs.is_none() {
                enqueue_audio_msg(
                    &mut sender,
                    audio::AudioMsg::SetParam(audio::VOICE_UFO, audio::PARAM_UFO_VARIANT, variant),
                    "ufo variant",
                )?;
                enqueue_audio_msg(
                    &mut sender,
                    audio::AudioMsg::Trigger(audio::VOICE_UFO),
                    "ufo trigger",
                )?;
            }
        }
        _ => {}
    }

    Ok(Some(AutomatedAudioCapture {
        path: path.clone(),
        handle: audio::spawn_captured_wav_writer(
            path.clone(),
            secs,
            runtime.sample_rate(),
            capture_consumer,
        ),
        runtime,
        sender,
        capture_xruns,
        thrust_release_deadline,
    }))
}

fn enqueue_audio_msg(
    sender: &mut audio::AudioMsgSender,
    msg: audio::AudioMsg,
    context: &str,
) -> Result<(), String> {
    sender
        .try_push(msg)
        .map(|_| ())
        .map_err(|error| format!("failed to enqueue {context} audio message: {error:?}"))
}

fn send_game_event_audio(sender: &mut audio::AudioMsgSender, event: GameEvent) {
    match event.kind {
        GameEventKind::BulletFired => {
            let _ = sender.try_push(audio::AudioMsg::Trigger(audio::VOICE_FIRE));
        }
        GameEventKind::BulletHitAsteroid => {
            let variant = event
                .asteroid_size
                .map(asteroid_size_audio_variant)
                .unwrap_or(0);
            let _ = sender.try_push(audio::AudioMsg::TriggerVariant(
                audio::VOICE_EXPLOSION,
                variant,
            ));
        }
        GameEventKind::UfoSirenOn => {
            let variant = event
                .ufo_variant
                .map(|variant| variant.audio_variant())
                .unwrap_or(0.0);
            let _ = sender.try_push(audio::AudioMsg::SetParam(
                audio::VOICE_UFO,
                audio::PARAM_UFO_VARIANT,
                variant,
            ));
            let _ = sender.try_push(audio::AudioMsg::Trigger(audio::VOICE_UFO));
        }
        GameEventKind::UfoSirenOff => {
            let _ = sender.try_push(audio::AudioMsg::Release(audio::VOICE_UFO));
        }
        GameEventKind::UfoDestroyed => {
            let _ = sender.try_push(audio::AudioMsg::TriggerVariant(audio::VOICE_EXPLOSION, 1));
        }
        _ => {}
    }
}

fn asteroid_size_audio_variant(size: AsteroidSize) -> u16 {
    match size {
        AsteroidSize::Large => 0,
        AsteroidSize::Medium => 1,
        AsteroidSize::Small => 2,
    }
}

fn heartbeat_curve_asteroid_count(elapsed: f32) -> u32 {
    let step = (elapsed / 2.5).floor() as u32;
    11_u32.saturating_sub(step).max(1)
}

fn write_audio_state_tick_event(
    writer: &mut BufWriter<File>,
    config: &RuntimeConfig,
    tick: u64,
    elapsed: f32,
    asteroid_count: u32,
) -> Result<(), String> {
    writeln!(
        writer,
        "{{\"tick\":{tick},\"time\":{elapsed:.6},\"event\":\"tick\",\"scenario\":\"{}\",\"seed\":{},\"asteroid_count\":{asteroid_count}}}",
        config.scenario.name(),
        config.seed.unwrap_or(0),
    )
    .map_err(|error| format!("failed to write state log: {error}"))
}

fn sleep_until(deadline: Instant) {
    let now = Instant::now();
    if deadline > now {
        thread::sleep(deadline - now);
    }
}

pub fn runtime_usage() -> String {
    "Usage: asteroids [--headless] [--screenshot <path>] [--capture-frames <N> --frames-out <dir>] [--audio-capture <secs> --wav-out <path>] [--seed <u64>] [--fixed-dt <secs>] [--simulate-secs <secs>] [--scenario <name>] [--xrun-log <path>] [--frame-time-log <path>] [--state-log <path>] [--bloom-intensity <value>] [--bloom-threshold <value>]\n\nScenarios: demo, idle, ship-spinning, horizontal-sweep, static-bright-line, static-bright-line-low-dwell, static-bright-line-high-dwell, gamma-ramp, thrust-1s, ship-spinning-with-thrust, heavy-input, asteroids-round-1, bullet-hit-asteroid, ship-collides-with-asteroid, lose-all-lives, explosion-storm, heartbeat-curve, fire-3, ufo-large, ufo-small, score-progression, eight-extra-lives, hyperspace-spam".to_string()
}

fn game_loop_for_scenario(scenario: Scenario, seed: Option<u64>) -> GameLoop {
    match scenario {
        Scenario::BulletHitAsteroid => {
            GameLoop::from_state(game::GameState::bullet_hit_asteroid_scenario(seed))
        }
        Scenario::ShipCollidesWithAsteroid => {
            GameLoop::from_state(game::GameState::ship_collides_with_asteroid_scenario(seed))
        }
        Scenario::LoseAllLives => {
            GameLoop::from_state(game::GameState::lose_all_lives_scenario(seed))
        }
        Scenario::UfoLarge => GameLoop::from_state(game::GameState::ufo_large_scenario(seed)),
        Scenario::UfoSmall => GameLoop::from_state(game::GameState::ufo_small_scenario(seed)),
        Scenario::ScoreProgression => {
            GameLoop::from_state(game::GameState::score_progression_scenario(seed))
        }
        Scenario::EightExtraLives => {
            GameLoop::from_state(game::GameState::eight_extra_lives_scenario(seed))
        }
        Scenario::HyperspaceSpam => {
            GameLoop::from_state(game::GameState::hyperspace_spam_scenario(seed))
        }
        _ => GameLoop::new_seeded(seed),
    }
}

fn render_tick(
    renderer: &mut HeadlessRenderer,
    config: &RuntimeConfig,
    fixed_dt: f32,
    tick_state: &mut TickState,
    game_loop: &mut GameLoop,
    io: TickIo<'_>,
) -> Result<(), String> {
    let start = Instant::now();
    let mut params = FrameParams::new(config.scenario, tick_state.sim_time, fixed_dt);
    let mut audio_sender = io.audio_sender;
    let mut substeps = 0;
    let mut dropped_accumulator_seconds = 0.0;
    if config.scenario.uses_game_simulation() {
        let input = scripted_controls(config.scenario, tick_state.sim_time);
        let report = game_loop.advance(fixed_dt, &input, |snapshot| {
            if let Some(sender) = audio_sender.as_mut() {
                let _ = sender.try_push(audio::AudioMsg::GameState(snapshot));
            }
        });
        substeps = report.substeps;
        dropped_accumulator_seconds = report.dropped_accumulator_seconds;
        params = params
            .with_asteroids(game_loop.interpolated_asteroids())
            .with_bullets(game_loop.interpolated_bullets())
            .with_ufo(game_loop.interpolated_ufo())
            .with_ufo_bullets(game_loop.interpolated_ufo_bullets())
            .with_game_over(game_loop.current().game_over)
            .with_readouts(game_loop.current().score, game_loop.current().lives);
        if let Some(ship) = game_loop.interpolated_ship_if_alive() {
            params = params.with_ship(ship);
        }
    }

    let events = game_loop.drain_events();
    if let Some(sender) = audio_sender.as_mut() {
        for event in events.iter().copied() {
            send_game_event_audio(sender, event);
        }
    }

    renderer.render(params)?;
    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
    if let Some(writer) = io.frame_time_log {
        writeln!(writer, "{duration_ms:.6}")
            .map_err(|error| format!("failed to write frame-time log: {error}"))?;
    }

    tick_state.advance(fixed_dt);
    if let Some(writer) = io.state_log {
        write_state_tick_event(
            writer,
            tick_state,
            config,
            game_loop,
            substeps,
            dropped_accumulator_seconds,
        )?;
        for event in events {
            write_state_event(writer, tick_state, config, &event.state_log_name())?;
        }
    } else {
        drop(events);
    }
    Ok(())
}

struct TickIo<'a> {
    audio_sender: Option<&'a mut audio::AudioMsgSender>,
    frame_time_log: Option<&'a mut BufWriter<File>>,
    state_log: Option<&'a mut BufWriter<File>>,
}

fn scripted_controls(scenario: Scenario, time_seconds: f32) -> ControlState {
    match scenario {
        Scenario::HeavyInput => game::heavy_input_controls(time_seconds),
        Scenario::BulletHitAsteroid => ControlState {
            fire: time_seconds < 0.02,
            ..ControlState::default()
        },
        Scenario::HyperspaceSpam => ControlState {
            hyperspace: hyperspace_spam_pressed(time_seconds),
            ..ControlState::default()
        },
        _ => ControlState::default(),
    }
}

fn hyperspace_spam_pressed(time_seconds: f32) -> bool {
    const PULSE_SECONDS: f32 = 0.02;
    [0.0, 0.25, 1.10, 1.25, 2.20]
        .into_iter()
        .any(|start| time_seconds >= start && time_seconds < start + PULSE_SECONDS)
}

fn write_state_event(
    writer: &mut BufWriter<File>,
    tick_state: &mut TickState,
    config: &RuntimeConfig,
    event: &str,
) -> Result<(), String> {
    let rng_outcome = tick_state.rng.next_u64();
    writeln!(
        writer,
        "{{\"tick\":{tick},\"time\":{sim_time:.6},\"event\":\"{event}\",\"scenario\":\"{}\",\"seed\":{},\"rng\":{rng_outcome}}}",
        config.scenario.name(),
        config.seed.unwrap_or(0),
        tick = tick_state.tick,
        sim_time = tick_state.sim_time,
    )
    .map_err(|error| format!("failed to write state log: {error}"))
}

fn write_state_tick_event(
    writer: &mut BufWriter<File>,
    tick_state: &mut TickState,
    config: &RuntimeConfig,
    game_loop: &GameLoop,
    substeps: u32,
    dropped_accumulator_seconds: f32,
) -> Result<(), String> {
    let rng_outcome = tick_state.rng.next_u64();
    let state = game_loop.current();
    (|| -> std::io::Result<()> {
        write!(
            writer,
            "{{\"tick\":{tick},\"time\":{sim_time:.6},\"event\":\"tick\",\"scenario\":\"{}\",\"seed\":{},\"rng\":{rng_outcome},\"physics_tick\":{},\"substeps\":{substeps},\"dropped_accumulator_seconds\":{dropped_accumulator_seconds:.6},\"ship_x\":{ship_x:.6},\"ship_y\":{ship_y:.6},\"ship_angle\":{ship_angle:.6},\"ship_vx\":{ship_vx:.6},\"ship_vy\":{ship_vy:.6}",
            config.scenario.name(),
            config.seed.unwrap_or(0),
            game_loop.tick(),
            tick = tick_state.tick,
            sim_time = tick_state.sim_time,
            ship_x = state.ship.position.x,
            ship_y = state.ship.position.y,
            ship_angle = state.ship.angle,
            ship_vx = state.ship.velocity.x,
            ship_vy = state.ship.velocity.y,
        )?;
        write_gameplay_fields(writer, state)?;
        write_asteroid_fields(writer, state)?;
        writeln!(writer, "}}")
    })()
    .map_err(|error| format!("failed to write state log: {error}"))
}

fn write_frame_metadata(
    writer: &mut BufWriter<File>,
    frame_index: usize,
    config: &RuntimeConfig,
    game_loop: &GameLoop,
) -> Result<(), String> {
    let state = game_loop.current();
    (|| -> std::io::Result<()> {
        write!(
            writer,
            "{{\"frame\":{frame_index},\"scenario\":\"{}\",\"seed\":{},\"physics_tick\":{}",
            config.scenario.name(),
            config.seed.unwrap_or(0),
            game_loop.tick(),
        )?;
        write_gameplay_fields(writer, state)?;
        write_asteroid_fields(writer, state)?;
        writeln!(writer, "}}")
    })()
    .map_err(|error| format!("failed to write frame metadata: {error}"))
}

fn write_gameplay_fields(
    writer: &mut BufWriter<File>,
    state: &game::GameState,
) -> std::io::Result<()> {
    write!(
        writer,
        ",\"alive\":{},\"lives\":{},\"score\":{},\"game_over\":{},\"bullet_count\":{},\"bullet_wrapped\":{},\"ufo_bullet_count\":{},\"ufo_bullet_wrapped\":{},\"bullets\":[",
        state.alive,
        state.lives,
        state.score,
        state.game_over,
        state.bullets.len(),
        state.any_bullet_wrapped_last_tick(),
        state.ufo_bullets.len(),
        state.any_ufo_bullet_wrapped_last_tick(),
    )?;
    for (index, bullet) in state.bullets.iter().enumerate() {
        if index > 0 {
            write!(writer, ",")?;
        }
        write!(
            writer,
            "{{\"id\":{},\"x\":{:.6},\"y\":{:.6},\"vx\":{:.6},\"vy\":{:.6},\"age\":{:.6},\"wrapped\":{}}}",
            bullet.id,
            bullet.position.x,
            bullet.position.y,
            bullet.velocity.x,
            bullet.velocity.y,
            bullet.age_seconds,
            bullet.wrapped_last_tick,
        )?;
    }
    write!(writer, "],\"ufo_bullets\":[")?;
    for (index, bullet) in state.ufo_bullets.iter().enumerate() {
        if index > 0 {
            write!(writer, ",")?;
        }
        write!(
            writer,
            "{{\"id\":{},\"x\":{:.6},\"y\":{:.6},\"vx\":{:.6},\"vy\":{:.6},\"age\":{:.6},\"wrapped\":{}}}",
            bullet.id,
            bullet.position.x,
            bullet.position.y,
            bullet.velocity.x,
            bullet.velocity.y,
            bullet.age_seconds,
            bullet.wrapped_last_tick,
        )?;
    }
    write!(writer, "]")?;
    if let Some(ufo) = state.ufo {
        write!(
            writer,
            ",\"ufo\":{{\"id\":{},\"variant\":\"{}\",\"x\":{:.6},\"y\":{:.6},\"vx\":{:.6},\"vy\":{:.6}}}",
            ufo.id,
            ufo.variant.name(),
            ufo.position.x,
            ufo.position.y,
            ufo.velocity.x,
            ufo.velocity.y,
        )
    } else {
        write!(writer, ",\"ufo\":null")
    }
}

fn write_asteroid_fields(
    writer: &mut BufWriter<File>,
    state: &game::GameState,
) -> std::io::Result<()> {
    let counts = state.asteroid_size_counts();
    write!(
        writer,
        ",\"round\":{},\"asteroid_count\":{},\"asteroids_large\":{},\"asteroids_medium\":{},\"asteroids_small\":{},\"asteroid_wrapped\":{},\"asteroids\":[",
        state.round,
        state.asteroid_count,
        counts.large,
        counts.medium,
        counts.small,
        state.any_asteroid_wrapped_last_tick(),
    )?;
    for (index, asteroid) in state.asteroids.iter().enumerate() {
        if index > 0 {
            write!(writer, ",")?;
        }
        write!(
            writer,
            "{{\"id\":{},\"size\":\"{}\",\"x\":{:.6},\"y\":{:.6},\"vx\":{:.6},\"vy\":{:.6},\"wrapped\":{}}}",
            asteroid.id,
            asteroid.size.name(),
            asteroid.position.x,
            asteroid.position.y,
            asteroid.velocity.x,
            asteroid.velocity.y,
            asteroid.wrapped_last_tick,
        )?;
    }
    write!(writer, "]")
}

struct TickState {
    tick: u64,
    sim_time: f32,
    rng: SeededRng,
}

impl TickState {
    fn new(seed: Option<u64>) -> Self {
        Self {
            tick: 0,
            sim_time: 0.0,
            rng: rng_for_seed(seed),
        }
    }

    fn advance(&mut self, fixed_dt: f32) {
        self.tick += 1;
        self.sim_time += fixed_dt;
    }
}

fn optional_writer(path: Option<&Path>) -> Result<Option<BufWriter<File>>, String> {
    path.map(|path| {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        File::create(path)
            .map(BufWriter::new)
            .map_err(|error| format!("failed to create {}: {error}", path.display()))
    })
    .transpose()
}

fn create_empty_file(path: &Path) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    File::create(path)
        .map(|_| ())
        .map_err(|error| format!("failed to create {}: {error}", path.display()))
}

fn frames_for_duration(duration_secs: f64, fixed_dt: f32) -> usize {
    (duration_secs / f64::from(fixed_dt)).ceil() as usize
}

fn capture_frame_digits(frame_count: usize) -> usize {
    frame_count.saturating_sub(1).to_string().len().max(2)
}

fn headless_render_size() -> PhysicalSize<u32> {
    env::var("ASTEROIDS_HEADLESS_SIZE")
        .ok()
        .and_then(|value| parse_size(&value))
        .or_else(size_from_capability_probe_notes)
        .unwrap_or_else(|| PhysicalSize::new(1920, 1080))
}

fn size_from_capability_probe_notes() -> Option<PhysicalSize<u32>> {
    let notes = fs::read_to_string("capability_probe/PROBE_NOTES.md").ok()?;
    for line in notes.lines() {
        if let Some((_, value)) = line.split_once("Physical resolution:")
            && let Some(size) = parse_size(value.trim())
        {
            return Some(size);
        }
    }
    None
}

fn parse_size(value: &str) -> Option<PhysicalSize<u32>> {
    let (width, height) = value.split_once('x')?;
    let width = width.trim().parse().ok()?;
    let height = height.trim().parse().ok()?;
    Some(PhysicalSize::new(width, height))
}

fn next_string(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn next_path(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<PathBuf, String> {
    Ok(PathBuf::from(next_string(args, flag)?))
}

fn next_value<T>(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = next_string(args, flag)?;
    value
        .parse()
        .map_err(|error| format!("{flag} value '{value}' is invalid: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_flags() {
        let config = RuntimeConfig::from_args(
            [
                "--headless",
                "--seed",
                "1",
                "--fixed-dt",
                "0.00694",
                "--scenario",
                "idle",
                "--screenshot",
                "/tmp/idle.png",
                "--bloom-intensity",
                "0.5",
                "--bloom-threshold",
                "0.25",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();

        assert!(config.headless);
        assert_eq!(config.seed, Some(1));
        assert_eq!(config.fixed_dt, Some(0.00694));
        assert_eq!(config.scenario, Scenario::Idle);
        assert_eq!(config.bloom_intensity, 0.5);
        assert_eq!(config.bloom_threshold, 0.25);
        assert!(!config.should_run_interactive());
    }

    #[test]
    fn capture_frame_digits_preserves_checkpoint_frame_name() {
        let width = capture_frame_digits(60);
        assert_eq!(width, 2);
        assert_eq!(format!("frame_{:0width$}.png", 0), "frame_00.png");
    }

    #[test]
    fn capture_frame_digits_expands_when_hundreds_are_present() {
        let width = capture_frame_digits(101);
        assert_eq!(width, 3);
        assert_eq!(format!("frame_{:0width$}.png", 100), "frame_100.png");
    }

    #[test]
    fn parses_combined_soul_visible_and_audible_scenario() {
        let config = RuntimeConfig::from_args(
            [
                "--headless",
                "--scenario",
                "ship-spinning-with-thrust",
                "--capture-frames",
                "144",
                "--frames-out",
                "/tmp/sa-frames",
                "--audio-capture",
                "10",
                "--wav-out",
                "/tmp/sa-audio.wav",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();

        assert_eq!(config.scenario, Scenario::ShipSpinningWithThrust);
        assert_eq!(config.capture_frames, Some(144));
        assert_eq!(config.audio_capture_secs, Some(10.0));
    }

    #[test]
    fn parses_heavy_input_physics_scenario() {
        let config = RuntimeConfig::from_args(
            [
                "--headless",
                "--scenario",
                "heavy-input",
                "--simulate-secs",
                "10",
                "--frame-time-log",
                "/tmp/loop-frametimes.log",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();

        assert_eq!(config.scenario, Scenario::HeavyInput);
        assert!(config.scenario.uses_game_simulation());
        assert_eq!(config.simulate_secs, Some(10.0));
    }

    #[test]
    fn parses_asteroids_round_one_scenario() {
        let config = RuntimeConfig::from_args(
            [
                "--headless",
                "--scenario",
                "asteroids-round-1",
                "--capture-frames",
                "144",
                "--frames-out",
                "/tmp/asteroids-round-1",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();

        assert_eq!(config.scenario, Scenario::AsteroidsRound1);
        assert!(config.scenario.uses_game_simulation());
    }

    #[test]
    fn parses_gameplay_verification_scenarios() {
        for name in [
            "bullet-hit-asteroid",
            "ship-collides-with-asteroid",
            "lose-all-lives",
            "score-progression",
            "eight-extra-lives",
            "hyperspace-spam",
        ] {
            let config = RuntimeConfig::from_args(
                ["--headless", "--scenario", name]
                    .into_iter()
                    .map(str::to_string),
            )
            .unwrap();

            assert!(config.scenario.uses_game_simulation());
        }
    }

    #[test]
    fn parses_scripted_audio_scenarios() {
        for name in [
            "explosion-storm",
            "heartbeat-curve",
            "fire-3",
            "ufo-large",
            "ufo-small",
        ] {
            let config = RuntimeConfig::from_args(
                [
                    "--headless",
                    "--scenario",
                    name,
                    "--audio-capture",
                    "1",
                    "--wav-out",
                    "/tmp/a.wav",
                ]
                .into_iter()
                .map(str::to_string),
            )
            .unwrap();

            assert!(config.scenario.uses_scripted_audio());
        }
    }
}
