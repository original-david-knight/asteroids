use std::{
    env,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use winit::dpi::PhysicalSize;

use crate::{
    audio,
    renderer::{FrameParams, HeadlessRenderer, Scenario},
    rng::{SeededRng, rng_for_seed},
    verify,
};

const DEFAULT_FIXED_DT_SECONDS: f32 = 1.0 / 144.0;

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
        Ok(())
    }
}

pub async fn run_automated(config: &RuntimeConfig) -> Result<(), String> {
    if let Some(path) = &config.xrun_log {
        create_empty_file(path)?;
    }
    let audio_writer =
        if let (Some(secs), Some(path)) = (config.audio_capture_secs, &config.wav_out) {
            Some((
                path.clone(),
                audio::spawn_silent_wav_writer(path.clone(), secs),
            ))
        } else {
            None
        };

    let fixed_dt = config.fixed_dt.unwrap_or(DEFAULT_FIXED_DT_SECONDS);
    let render_size = headless_render_size();
    let mut renderer = HeadlessRenderer::new(render_size).await?;
    eprintln!(
        "headless render: {}x{}, phosphor={:?}, scenario={}, seed={}",
        renderer.size().width,
        renderer.size().height,
        renderer.phosphor_format(),
        config.scenario.name(),
        config.seed.unwrap_or(0)
    );

    let mut frame_time_log = optional_writer(config.frame_time_log.as_deref())?;
    let mut state_log = optional_writer(config.state_log.as_deref())?;
    let mut tick_state = TickState::new(config.seed);

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
            frame_time_log.as_mut(),
            state_log.as_mut(),
        )?;
    }

    if let (Some(frame_count), Some(frames_out)) = (config.capture_frames, &config.frames_out) {
        fs::create_dir_all(frames_out)
            .map_err(|error| format!("failed to create {}: {error}", frames_out.display()))?;
        let frame_digits = capture_frame_digits(frame_count);
        for frame_index in 0..frame_count {
            render_tick(
                &mut renderer,
                config,
                fixed_dt,
                &mut tick_state,
                frame_time_log.as_mut(),
                state_log.as_mut(),
            )?;
            let rgba = renderer.capture_rgba8()?;
            let path = frames_out.join(format!("frame_{frame_index:0frame_digits$}.png"));
            verify::save_png(&path, renderer.size().width, renderer.size().height, &rgba)?;
        }
    }

    if let Some(path) = &config.screenshot {
        if simulate_frames == 0 && config.capture_frames.is_none() {
            render_tick(
                &mut renderer,
                config,
                fixed_dt,
                &mut tick_state,
                frame_time_log.as_mut(),
                state_log.as_mut(),
            )?;
        }
        let rgba = renderer.capture_rgba8()?;
        verify::save_png(path, renderer.size().width, renderer.size().height, &rgba)?;
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
            frame_time_log.as_mut(),
            state_log.as_mut(),
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
    if let Some((path, handle)) = audio_writer {
        handle
            .join()
            .map_err(|_| format!("silent WAV writer panicked for {}", path.display()))?
            .map_err(|error| format!("failed to write silent WAV {}: {error}", path.display()))?;
    }
    Ok(())
}

pub fn runtime_usage() -> String {
    "Usage: asteroids [--headless] [--screenshot <path>] [--capture-frames <N> --frames-out <dir>] [--audio-capture <secs> --wav-out <path>] [--seed <u64>] [--fixed-dt <secs>] [--simulate-secs <secs>] [--scenario <name>] [--xrun-log <path>] [--frame-time-log <path>] [--state-log <path>]\n\nScenarios: demo, idle, horizontal-sweep, static-bright-line, static-bright-line-low-dwell, static-bright-line-high-dwell, gamma-ramp".to_string()
}

fn render_tick(
    renderer: &mut HeadlessRenderer,
    config: &RuntimeConfig,
    fixed_dt: f32,
    tick_state: &mut TickState,
    frame_time_log: Option<&mut BufWriter<File>>,
    state_log: Option<&mut BufWriter<File>>,
) -> Result<(), String> {
    let start = Instant::now();
    renderer.render(FrameParams::new(
        config.scenario,
        tick_state.sim_time,
        fixed_dt,
    ))?;
    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
    if let Some(writer) = frame_time_log {
        writeln!(writer, "{duration_ms:.6}")
            .map_err(|error| format!("failed to write frame-time log: {error}"))?;
    }

    tick_state.advance(fixed_dt);
    if let Some(writer) = state_log {
        write_state_event(writer, tick_state, config, "tick")?;
    }
    Ok(())
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
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();

        assert!(config.headless);
        assert_eq!(config.seed, Some(1));
        assert_eq!(config.fixed_dt, Some(0.00694));
        assert_eq!(config.scenario, Scenario::Idle);
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
}
