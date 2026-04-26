use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
};

use asteroids::verify;

const SUBCOMMANDS: &[&str] = &[
    "decay-fit",
    "line-glow-width",
    "line-peak-luminance",
    "gamma-ramp",
    "banding",
    "peak-count",
    "ship-outline",
    "soul-visible",
    "asteroid-count",
    "screen-wrap",
    "lives-display",
    "playfield-rect",
    "audio-rms",
    "audio-dominant-freq",
    "audio-band-energy",
    "heartbeat-tempo",
    "xrun-count",
    "frame-time-p99",
    "state-trace",
];

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        println!("{}", general_help());
        return Ok(());
    }

    let command = args.remove(0);
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{}", subcommand_help(&command));
        return Ok(());
    }

    let mut args = CliArgs::new(args);
    match command.as_str() {
        "decay-fit" => cmd_decay_fit(&mut args),
        "line-glow-width" => cmd_line_glow_width(&mut args),
        "line-peak-luminance" => cmd_line_peak_luminance(&mut args),
        "gamma-ramp" => cmd_gamma_ramp(&mut args),
        "banding" => cmd_banding(&mut args),
        "peak-count" => cmd_peak_count(&mut args),
        "audio-rms" => cmd_audio_rms(&mut args),
        "audio-dominant-freq" => cmd_audio_dominant_freq(&mut args),
        "audio-band-energy" => cmd_audio_band_energy(&mut args),
        "xrun-count" => cmd_xrun_count(&mut args),
        "frame-time-p99" => cmd_frame_time_p99(&mut args),
        "state-trace" => cmd_state_trace(&mut args),
        "ship-outline" | "soul-visible" | "asteroid-count" | "screen-wrap" | "lives-display"
        | "playfield-rect" | "heartbeat-tempo" => Err(format!(
            "{command} is reserved for a later gameplay/audio milestone\n\n{}",
            subcommand_help(&command)
        )),
        _ => Err(format!(
            "unknown subcommand '{command}'\n\n{}",
            general_help()
        )),
    }
}

fn cmd_decay_fit(args: &mut CliArgs) -> Result<(), String> {
    let frames = args.path("--frames")?;
    let tau_min = args.value_or("--tau-min", 0.0)?;
    let tau_max = args.value_or("--tau-max", f64::INFINITY)?;
    let r2_min = args.value_or("--r2-min", 0.0)?;
    let fixed_dt = args.value_or("--fixed-dt", 0.00694_f64)?;
    args.finish()?;

    let fit = decay_fit_from_frames(&frames, fixed_dt)?;
    if fit.tau_seconds < tau_min || fit.tau_seconds > tau_max || fit.r_squared < r2_min {
        return Err(format!(
            "decay-fit failed: tau={:.6}s r2={:.6}, expected tau=[{tau_min:.6},{tau_max:.6}] r2>={r2_min:.6}",
            fit.tau_seconds, fit.r_squared
        ));
    }
    println!(
        "decay-fit ok: tau={:.6}s r2={:.6}",
        fit.tau_seconds, fit.r_squared
    );
    Ok(())
}

fn cmd_line_glow_width(args: &mut CliArgs) -> Result<(), String> {
    let lo = verify::load_png(&args.path("--lo")?)?;
    let hi = verify::load_png(&args.path("--hi")?)?;
    let ratio_min = args.value_or("--fwhm-ratio-min", 1.0_f32)?;
    let core_peak_tolerance = args.value_or("--core-peak-tolerance", f32::INFINITY)?;
    args.finish()?;

    let lo_width = estimated_fwhm_width(&lo);
    let hi_width = estimated_fwhm_width(&hi);
    let ratio = hi_width / lo_width.max(1.0);
    let lo_peak = lo.max_luminance();
    let hi_peak = hi.max_luminance();
    let peak_delta = if lo_peak <= f32::EPSILON {
        0.0
    } else {
        ((hi_peak - lo_peak) / lo_peak).abs()
    };

    if ratio < ratio_min || peak_delta > core_peak_tolerance {
        return Err(format!(
            "line-glow-width failed: lo_width={lo_width:.3} hi_width={hi_width:.3} ratio={ratio:.3} peak_delta={peak_delta:.3}"
        ));
    }
    println!("line-glow-width ok: lo_width={lo_width:.3} hi_width={hi_width:.3} ratio={ratio:.3}");
    Ok(())
}

fn cmd_line_peak_luminance(args: &mut CliArgs) -> Result<(), String> {
    let image = verify::load_png(&args.path("--frame")?)?;
    let min = args.value_or("--min", f32::NEG_INFINITY)?;
    let max = args.value_or("--max", f32::INFINITY)?;
    args.finish()?;
    let peak = image.max_luminance();
    if peak < min || peak > max {
        return Err(format!(
            "line-peak-luminance failed: peak={peak:.6}, expected [{min:.6},{max:.6}]"
        ));
    }
    println!("line-peak-luminance ok: peak={peak:.6}");
    Ok(())
}

fn cmd_gamma_ramp(args: &mut CliArgs) -> Result<(), String> {
    let image = verify::load_png(&args.path("--frame")?)?;
    let min_steps = args.value_or("--min-steps", 8_usize)?;
    args.finish()?;
    let values = verify::line_luminance(
        &image,
        (0.0, image.height as f32 * 0.5),
        (
            image.width.saturating_sub(1) as f32,
            image.height as f32 * 0.5,
        ),
        image.width.min(512) as usize,
    );
    let increasing = values.windows(2).filter(|pair| pair[1] >= pair[0]).count();
    if increasing < min_steps {
        return Err(format!(
            "gamma-ramp failed: monotonic_steps={increasing}, expected at least {min_steps}"
        ));
    }
    println!("gamma-ramp ok: monotonic_steps={increasing}");
    Ok(())
}

fn cmd_banding(args: &mut CliArgs) -> Result<(), String> {
    let image = verify::load_png(&args.path("--frame")?)?;
    let max_step_jump = args.value("--max-step-jump")?;
    args.finish()?;
    let values = verify::line_luminance(
        &image,
        (0.0, image.height as f32 * 0.5),
        (
            image.width.saturating_sub(1) as f32,
            image.height as f32 * 0.5,
        ),
        image.width.min(1024) as usize,
    );
    let max_jump = values
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0_f32, f32::max);
    if max_jump > max_step_jump {
        return Err(format!(
            "banding failed: max_step_jump={max_jump:.6}, expected <= {max_step_jump:.6}"
        ));
    }
    println!("banding ok: max_step_jump={max_jump:.6}");
    Ok(())
}

fn cmd_peak_count(args: &mut CliArgs) -> Result<(), String> {
    let image = verify::load_png(&args.path("--frame")?)?;
    let threshold = args.value_or("--threshold", 0.1_f32)?;
    let min = args.value_or("--min", 0_usize)?;
    let max = args.value_or("--max", usize::MAX)?;
    args.finish()?;
    let values = verify::line_luminance(
        &image,
        (0.0, image.height as f32 * 0.5),
        (
            image.width.saturating_sub(1) as f32,
            image.height as f32 * 0.5,
        ),
        image.width.min(2048) as usize,
    );
    let count = verify::peak_count(&values, threshold);
    if count < min || count > max {
        return Err(format!(
            "peak-count failed: count={count}, expected [{min},{max}], threshold={threshold}"
        ));
    }
    println!("peak-count ok: count={count}");
    Ok(())
}

fn cmd_audio_rms(args: &mut CliArgs) -> Result<(), String> {
    let wav = verify::load_wav(&args.path("--wav")?)?;
    let min = args.value_or("--min", f32::NEG_INFINITY)?;
    let max = args.value_or("--max", f32::INFINITY)?;
    args.finish()?;
    let rms = verify::rms(&wav.samples);
    if rms < min || rms > max {
        return Err(format!(
            "audio-rms failed: rms={rms:.6}, expected [{min:.6},{max:.6}]"
        ));
    }
    println!("audio-rms ok: rms={rms:.6}");
    Ok(())
}

fn cmd_audio_dominant_freq(args: &mut CliArgs) -> Result<(), String> {
    let wav = verify::load_wav(&args.path("--wav")?)?;
    let min_hz = args.value_or("--min-hz", 0.0_f32)?;
    let max_hz = args.value_or("--max-hz", f32::INFINITY)?;
    args.finish()?;
    let freq = verify::dominant_freq(&wav).unwrap_or(0.0);
    if freq < min_hz || freq > max_hz {
        return Err(format!(
            "audio-dominant-freq failed: freq={freq:.3}Hz, expected [{min_hz:.3},{max_hz:.3}]"
        ));
    }
    println!("audio-dominant-freq ok: freq={freq:.3}Hz");
    Ok(())
}

fn cmd_audio_band_energy(args: &mut CliArgs) -> Result<(), String> {
    let wav = verify::load_wav(&args.path("--wav")?)?;
    let lo_hz = args.value("--lo-hz")?;
    let hi_hz = args.value("--hi-hz")?;
    let min_fraction = args.value_or("--min-fraction", 0.0_f32)?;
    args.finish()?;
    let fraction = verify::spectral_band_energy(&wav, lo_hz, hi_hz);
    if fraction < min_fraction {
        return Err(format!(
            "audio-band-energy failed: fraction={fraction:.6}, expected >= {min_fraction:.6}"
        ));
    }
    println!("audio-band-energy ok: fraction={fraction:.6}");
    Ok(())
}

fn cmd_xrun_count(args: &mut CliArgs) -> Result<(), String> {
    let count = verify::xrun_count(&args.path("--log")?)?;
    let max = args.value_or("--max", usize::MAX)?;
    args.finish()?;
    if count > max {
        return Err(format!(
            "xrun-count failed: count={count}, expected <= {max}"
        ));
    }
    println!("xrun-count ok: count={count}");
    Ok(())
}

fn cmd_frame_time_p99(args: &mut CliArgs) -> Result<(), String> {
    let p99 = verify::frame_time_p99(&args.path("--log")?)?;
    let max_ms = args.value("--max-ms")?;
    args.finish()?;
    if p99 > max_ms {
        return Err(format!(
            "frame-time-p99 failed: p99={p99:.6}ms, expected <= {max_ms:.6}ms"
        ));
    }
    println!("frame-time-p99 ok: p99={p99:.6}ms");
    Ok(())
}

fn cmd_state_trace(args: &mut CliArgs) -> Result<(), String> {
    let events = verify::load_state_trace(&args.path("--log")?)?;
    let expected = args
        .optional_string("--expect")?
        .map(|value| {
            value
                .split(',')
                .filter(|name| !name.trim().is_empty())
                .map(|name| name.trim().to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    args.finish()?;
    let missing = verify::state_trace_contains(&events, &expected);
    if !missing.is_empty() {
        return Err(format!(
            "state-trace failed: missing expected events {}",
            missing.join(",")
        ));
    }
    println!("state-trace ok: events={}", events.len());
    Ok(())
}

fn decay_fit_from_frames(dir: &Path, fixed_dt: f64) -> Result<verify::DecayFit, String> {
    let frames = sorted_frame_paths(dir)?;
    if frames.len() < 3 {
        return Err(format!(
            "{} contains fewer than 3 frame_*.png files",
            dir.display()
        ));
    }
    let first = verify::load_png(&frames[0])?;
    let (peak_x, peak_y, peak_luma) = brightest_pixel(&first);
    if peak_luma <= 0.0 {
        return Err(format!("first frame in {} has no luminance", dir.display()));
    }

    let mut coords = Vec::with_capacity(first.width as usize * 5 + 5);
    for y_offset in -2_i32..=2 {
        let y = (peak_y as i32 + y_offset).clamp(0, first.height.saturating_sub(1) as i32) as u32;
        for x in 0..first.width {
            coords.push((x, y));
        }
    }
    for y in 0..first.height {
        coords.push((peak_x, y));
    }
    coords.sort_unstable();
    coords.dedup();

    let mut traces = vec![Vec::with_capacity(frames.len()); coords.len()];
    for frame in &frames {
        let image = verify::load_png(frame)?;
        for (trace, (x, y)) in traces.iter_mut().zip(coords.iter().copied()) {
            trace.push(image.luminance(
                x.min(image.width.saturating_sub(1)),
                y.min(image.height.saturating_sub(1)),
            ));
        }
    }

    let mut best = None;
    for trace in traces {
        let Some((peak_index, peak)) = trace
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
        else {
            continue;
        };
        if peak <= 0.05 || peak_index + 3 >= trace.len() {
            continue;
        }
        let samples: Vec<_> = trace[peak_index..]
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, value)| *value > peak * 0.03)
            .map(|(i, value)| (i as f64 * fixed_dt, f64::from(value)))
            .collect();
        if let Some(fit) = verify::decay_fit(&samples)
            && best
                .map(|current: verify::DecayFit| fit.r_squared > current.r_squared)
                .unwrap_or(true)
        {
            best = Some(fit);
        }
    }
    best.ok_or_else(|| format!("no decaying luminance trace found in {}", dir.display()))
}

fn sorted_frame_paths(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = fs::read_dir(dir)
        .map_err(|error| format!("failed to read {}: {error}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("frame_") && name.ends_with(".png"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(frame_index)
            .unwrap_or(usize::MAX)
    });
    Ok(paths)
}

fn frame_index(file_name: &str) -> Option<usize> {
    file_name
        .strip_prefix("frame_")?
        .strip_suffix(".png")?
        .parse()
        .ok()
}

fn brightest_pixel(image: &verify::PngImage) -> (u32, u32, f32) {
    let mut best = (0, 0, 0.0);
    for y in 0..image.height {
        for x in 0..image.width {
            let luma = image.luminance(x, y);
            if luma > best.2 {
                best = (x, y, luma);
            }
        }
    }
    best
}

fn estimated_fwhm_width(image: &verify::PngImage) -> f32 {
    let best = brightest_pixel(image);
    let threshold = best.2 * 0.5;
    let row_width = (0..image.width)
        .filter(|x| image.luminance(*x, best.1) >= threshold)
        .count() as f32;
    let col_width = (0..image.height)
        .filter(|y| image.luminance(best.0, *y) >= threshold)
        .count() as f32;
    row_width.max(col_width)
}

fn general_help() -> String {
    format!(
        "Usage: cargo run --bin verify -- <subcommand> [args]\n\nSubcommands:\n  {}",
        SUBCOMMANDS.join("\n  ")
    )
}

fn subcommand_help(command: &str) -> String {
    match command {
        "decay-fit" => "Usage: verify decay-fit --frames <dir> --tau-min <secs> --tau-max <secs> --r2-min <value> [--fixed-dt <secs>]".to_string(),
        "line-glow-width" => "Usage: verify line-glow-width --lo <png> --hi <png> --fwhm-ratio-min <ratio> [--core-peak-tolerance <fraction>]".to_string(),
        "line-peak-luminance" => "Usage: verify line-peak-luminance --frame <png> --min <luma> --max <luma>".to_string(),
        "gamma-ramp" => "Usage: verify gamma-ramp --frame <png> [--min-steps <n>]".to_string(),
        "banding" => "Usage: verify banding --frame <png> --max-step-jump <luma>".to_string(),
        "peak-count" => "Usage: verify peak-count --frame <png> [--threshold <luma>] [--min <n>] [--max <n>]".to_string(),
        "ship-outline" => "Usage: verify ship-outline --frames <dir> --vertex-count <n> --rotation-rate-rad-per-sec <rate> --tolerance <value>".to_string(),
        "soul-visible" => "Usage: verify soul-visible [task-specific args added by the soul-visible milestone]".to_string(),
        "asteroid-count" => "Usage: verify asteroid-count --frame <png> --count <n>".to_string(),
        "screen-wrap" => "Usage: verify screen-wrap --log <state-jsonl> --expect <event,...>".to_string(),
        "lives-display" => "Usage: verify lives-display --frame <png> --max-displayed <n> --state-log <state-jsonl>".to_string(),
        "playfield-rect" => "Usage: verify playfield-rect --frame <png> --aspect <w:h> [--centered] [--bezel-min-fraction <value>]".to_string(),
        "audio-rms" => "Usage: verify audio-rms --wav <wav> --min <value> [--max <value>]".to_string(),
        "audio-dominant-freq" => "Usage: verify audio-dominant-freq --wav <wav> --min-hz <hz> --max-hz <hz>".to_string(),
        "audio-band-energy" => "Usage: verify audio-band-energy --wav <wav> --lo-hz <hz> --hi-hz <hz> --min-fraction <value>".to_string(),
        "heartbeat-tempo" => "Usage: verify heartbeat-tempo --wav <wav> --state-log <state-jsonl> --tempo-curve <name> --tolerance <value>".to_string(),
        "xrun-count" => "Usage: verify xrun-count --log <path> --max <n>".to_string(),
        "frame-time-p99" => "Usage: verify frame-time-p99 --log <path> --max-ms <ms>".to_string(),
        "state-trace" => "Usage: verify state-trace --log <state-jsonl> [--expect <event,event,...>]".to_string(),
        _ => format!("unknown subcommand '{command}'"),
    }
}

struct CliArgs {
    args: Vec<String>,
}

impl CliArgs {
    fn new(args: Vec<String>) -> Self {
        Self { args }
    }

    fn path(&mut self, flag: &str) -> Result<PathBuf, String> {
        self.optional_string(flag)?
            .map(PathBuf::from)
            .ok_or_else(|| format!("{flag} is required"))
    }

    fn value<T>(&mut self, flag: &str) -> Result<T, String>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        self.optional_string(flag)?
            .ok_or_else(|| format!("{flag} is required"))?
            .parse()
            .map_err(|error| format!("{flag} has invalid value: {error}"))
    }

    fn value_or<T>(&mut self, flag: &str, default: T) -> Result<T, String>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        match self.optional_string(flag)? {
            Some(value) => value
                .parse()
                .map_err(|error| format!("{flag} has invalid value: {error}")),
            None => Ok(default),
        }
    }

    fn optional_string(&mut self, flag: &str) -> Result<Option<String>, String> {
        let Some(index) = self.args.iter().position(|arg| arg == flag) else {
            return Ok(None);
        };
        self.args.remove(index);
        if index >= self.args.len() {
            return Err(format!("{flag} requires a value"));
        }
        Ok(Some(self.args.remove(index)))
    }

    fn finish(&self) -> Result<(), String> {
        if self.args.is_empty() {
            Ok(())
        } else {
            Err(format!("unexpected arguments: {}", self.args.join(" ")))
        }
    }
}
