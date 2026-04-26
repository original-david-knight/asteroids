use std::{
    env,
    f32::consts::PI,
    fs,
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use asteroids::{audio, highscore, verify};
use serde_json::Value;

const SUBCOMMANDS: &[&str] = &[
    "decay-fit",
    "line-glow-width",
    "line-peak-luminance",
    "gamma-ramp",
    "banding",
    "peak-count",
    "ship-outline",
    "trail-luminance",
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
        "ship-outline" => cmd_ship_outline(&mut args),
        "trail-luminance" => cmd_trail_luminance(&mut args),
        "playfield-rect" => cmd_playfield_rect(&mut args),
        "audio-rms" => cmd_audio_rms(&mut args),
        "audio-dominant-freq" => cmd_audio_dominant_freq(&mut args),
        "audio-band-energy" => cmd_audio_band_energy(&mut args),
        "xrun-count" => cmd_xrun_count(&mut args),
        "frame-time-p99" => cmd_frame_time_p99(&mut args),
        "state-trace" => cmd_state_trace(&mut args),
        "soul-visible" => cmd_soul_visible(&mut args),
        "asteroid-count" => cmd_asteroid_count(&mut args),
        "screen-wrap" => cmd_screen_wrap(&mut args),
        "heartbeat-tempo" => cmd_heartbeat_tempo(&mut args),
        "lives-display" => cmd_lives_display(&mut args),
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
    let gamma = args.value_or("--gamma", 2.2_f32)?;
    let tolerance = args.value_or("--tolerance", 0.05_f32)?;
    let min_steps = args.value_or("--min-steps", 8_usize)?;
    args.finish()?;
    if !gamma.is_finite() || gamma <= 0.0 {
        return Err("--gamma must be a finite positive number".to_string());
    }
    if !tolerance.is_finite() || tolerance < 0.0 {
        return Err("--tolerance must be a finite non-negative number".to_string());
    }

    let playfield = centered_aspect_rect(image.width, image.height, PLAYFIELD_ASPECT_RATIO)?;
    let samples = gamma_ramp_samples(&image, playfield, gamma);
    let increasing = samples
        .windows(2)
        .filter(|pair| pair[1].actual + tolerance >= pair[0].actual)
        .count();
    if increasing < min_steps.min(samples.len().saturating_sub(1)) {
        return Err(format!(
            "gamma-ramp failed: monotonic_steps={increasing}, expected at least {min_steps}"
        ));
    }

    let mut max_error = 0.0_f32;
    let mut worst_bar = 0;
    for sample in &samples {
        let error = (sample.actual - sample.expected).abs();
        if error > max_error {
            max_error = error;
            worst_bar = sample.index;
        }
    }
    if max_error > tolerance {
        let sample = &samples[worst_bar];
        return Err(format!(
            "gamma-ramp failed: bar={} input={:.3} actual={:.6} expected={:.6} error={max_error:.6}, tolerance={tolerance:.6}",
            sample.index, sample.input, sample.actual, sample.expected
        ));
    }
    println!(
        "gamma-ramp ok: bars={} gamma={gamma:.3} max_error={max_error:.6} worst_bar={worst_bar}",
        samples.len()
    );
    Ok(())
}

fn cmd_banding(args: &mut CliArgs) -> Result<(), String> {
    let image = verify::load_png(&args.path("--frame")?)?;
    let max_step_jump = args.value("--max-step-jump")?;
    args.finish()?;
    let values = banding_luminance_values(&image);
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

fn cmd_ship_outline(args: &mut CliArgs) -> Result<(), String> {
    let frames = args.path("--frames")?;
    let vertex_count: usize = args.value("--vertex-count")?;
    let rotation_rate: f32 = args.value("--rotation-rate-rad-per-sec")?;
    let tolerance: f32 = args.value("--tolerance")?;
    args.finish()?;

    if vertex_count != SHIP_VERTEX_COUNT {
        return Err(format!(
            "ship-outline failed: verifier recognizes {SHIP_VERTEX_COUNT} ship vertices, expected {vertex_count}"
        ));
    }

    let frames = sorted_frame_paths(&frames)?;
    if frames.len() < 2 {
        return Err("ship-outline failed: at least two frames are required".to_string());
    }

    let detections = detect_ship_angles(&frames, rotation_rate)?;
    let fit = fit_angle_rate(&detections, VERIFY_FIXED_DT_SECONDS);
    let rate_error = (fit.rate_rad_per_sec - rotation_rate).abs();
    if rate_error > tolerance {
        return Err(format!(
            "ship-outline failed: rate={:.6}rad/s expected={rotation_rate:.6} tolerance={tolerance:.6}, max_residual={:.6}rad",
            fit.rate_rad_per_sec, fit.max_residual_rad
        ));
    }

    let weakest_vertex = detections
        .iter()
        .flat_map(|detection| detection.vertex_luminance)
        .fold(f32::INFINITY, f32::min);
    println!(
        "ship-outline ok: frames={} vertices={vertex_count}, rate={:.6}rad/s, max_residual={:.6}rad, weakest_vertex_luma={weakest_vertex:.6}",
        detections.len(),
        fit.rate_rad_per_sec,
        fit.max_residual_rad
    );
    Ok(())
}

fn cmd_trail_luminance(args: &mut CliArgs) -> Result<(), String> {
    let frames = args.path("--frames")?;
    let behind_vector = args.flag("--behind-vector");
    let min_luminance: f32 = args.value("--min-luminance")?;
    args.finish()?;
    if !behind_vector {
        return Err("trail-luminance requires --behind-vector".to_string());
    }

    let frames = sorted_frame_paths(&frames)?;
    if frames.len() < 8 {
        return Err("trail-luminance failed: at least eight frames are required".to_string());
    }

    let detections =
        detect_ship_angles(&frames, asteroids::tuning::SHIP_ROTATION_RATE_RAD_PER_SEC)?;
    let best_luminance = ship_trail_luminance(&frames, &detections)?;
    if best_luminance < min_luminance {
        return Err(format!(
            "trail-luminance failed: behind-vector luminance={best_luminance:.6}, expected >= {min_luminance:.6}"
        ));
    }
    println!("trail-luminance ok: behind-vector luminance={best_luminance:.6}");
    Ok(())
}

fn cmd_soul_visible(args: &mut CliArgs) -> Result<(), String> {
    let frames = args.path("--frames")?;
    let rotation_tolerance: f32 = args.value_or(
        "--rotation-tolerance",
        SOUL_VISIBLE_ROTATION_TOLERANCE_RAD_PER_SEC,
    )?;
    let min_trail_luminance: f32 =
        args.value_or("--min-trail-luminance", SOUL_VISIBLE_MIN_TRAIL_LUMINANCE)?;
    args.finish()?;

    let frames = sorted_frame_paths(&frames)?;
    if frames.len() < SOUL_VISIBLE_MIN_FRAMES {
        return Err(format!(
            "soul-visible failed: at least {SOUL_VISIBLE_MIN_FRAMES} frames are required, found {}",
            frames.len()
        ));
    }

    let rotation_rate = asteroids::tuning::SHIP_ROTATION_RATE_RAD_PER_SEC;
    let detections = detect_ship_angles(&frames, rotation_rate)?;
    let fit = fit_angle_rate(&detections, VERIFY_FIXED_DT_SECONDS);
    let rate_error = (fit.rate_rad_per_sec - rotation_rate).abs();
    if rate_error > rotation_tolerance {
        return Err(format!(
            "soul-visible failed: rate={:.6}rad/s expected={rotation_rate:.6} tolerance={rotation_tolerance:.6}, max_residual={:.6}rad",
            fit.rate_rad_per_sec, fit.max_residual_rad
        ));
    }

    let weakest_vertex = detections
        .iter()
        .flat_map(|detection| detection.vertex_luminance)
        .fold(f32::INFINITY, f32::min);
    let best_luminance = ship_trail_luminance(&frames, &detections)?;
    let trail_floor = min_trail_luminance.max(f32::EPSILON);
    if best_luminance < trail_floor {
        return Err(format!(
            "soul-visible failed: trail_luminance={best_luminance:.6}, expected >= {trail_floor:.6}"
        ));
    }

    println!(
        "soul-visible ok: frames={} vertices={SHIP_VERTEX_COUNT}, rate={:.6}rad/s, max_residual={:.6}rad, weakest_vertex_luma={weakest_vertex:.6}, trail_luminance={best_luminance:.6}",
        detections.len(),
        fit.rate_rad_per_sec,
        fit.max_residual_rad,
    );
    Ok(())
}

fn ship_trail_luminance(frames: &[PathBuf], detections: &[ShipDetection]) -> Result<f32, String> {
    let image = verify::load_png(
        frames
            .last()
            .ok_or_else(|| "trail-luminance failed: no frames loaded".to_string())?,
    )?;
    let angle = detections
        .last()
        .ok_or_else(|| "trail-luminance failed: no ship detections".to_string())?
        .angle_rad;
    let playfield = centered_aspect_rect(image.width, image.height, PLAYFIELD_ASPECT_RATIO)?;
    let lag_angle =
        asteroids::tuning::SHIP_ROTATION_RATE_RAD_PER_SEC * VERIFY_FIXED_DT_SECONDS * 6.0;
    let mut best_luminance = 0.0_f32;

    for (start, end) in SHIP_SEGMENTS {
        let local_start = SHIP_VERTICES[start] * asteroids::tuning::SHIP_SPINNING_SCALE;
        let local_end = SHIP_VERTICES[end] * asteroids::tuning::SHIP_SPINNING_SCALE;
        for sample in [0.25_f32, 0.50, 0.75] {
            let local = local_start + (local_end - local_start) * sample;
            let current = gameplay_to_pixel(playfield, rotate_vec2(local, angle));
            let behind = gameplay_to_pixel(playfield, rotate_vec2(local, angle - lag_angle));
            let trail_point = (
                current.0 + (behind.0 - current.0) * 1.15,
                current.1 + (behind.1 - current.1) * 1.15,
            );
            best_luminance = best_luminance.max(max_luminance_near(&image, trail_point, 4));
        }
    }

    Ok(best_luminance)
}

fn cmd_playfield_rect(args: &mut CliArgs) -> Result<(), String> {
    let image = verify::load_png(&args.path("--frame")?)?;
    let aspect = parse_aspect_ratio(&args.value_or("--aspect", "4:3".to_string())?)?;
    let centered = args.flag("--centered");
    let bezel_min_fraction = args.value_or("--bezel-min-fraction", 0.0_f32)?;
    args.finish()?;

    let rect = centered_aspect_rect(image.width, image.height, aspect)?;
    let actual_aspect = rect.width() as f32 / rect.height().max(1) as f32;
    let aspect_error = ((actual_aspect - aspect) / aspect).abs();
    if aspect_error > 0.002 {
        return Err(format!(
            "playfield-rect failed: rect aspect={actual_aspect:.6}, expected {aspect:.6}"
        ));
    }

    let left_margin = rect.left as f32 / image.width.max(1) as f32;
    let right_margin = image.width.saturating_sub(rect.right) as f32 / image.width.max(1) as f32;
    if centered && (left_margin - right_margin).abs() > (1.5 / image.width.max(1) as f32) {
        return Err(format!(
            "playfield-rect failed: margins not centered left={left_margin:.6} right={right_margin:.6}"
        ));
    }
    if left_margin < bezel_min_fraction || right_margin < bezel_min_fraction {
        return Err(format!(
            "playfield-rect failed: bezel margins left={left_margin:.6} right={right_margin:.6}, expected >= {bezel_min_fraction:.6}"
        ));
    }

    let peak = image.max_luminance();
    if peak <= 0.0 {
        return Err("playfield-rect failed: frame has no visible beam signal".to_string());
    }
    let threshold = (peak * 0.08).max(0.015);
    let min_signal_pixels = 12;
    let left_signal = signal_count(&image, 0, rect.left, 0, image.height, threshold);
    let right_signal = signal_count(&image, rect.right, image.width, 0, image.height, threshold);
    let playfield_signal = signal_count(
        &image,
        rect.left,
        rect.right,
        rect.top,
        rect.bottom,
        threshold,
    );

    if left_signal < min_signal_pixels || right_signal < min_signal_pixels {
        return Err(format!(
            "playfield-rect failed: bezel readout signal too low left={left_signal} right={right_signal}, threshold={threshold:.6}"
        ));
    }
    if playfield_signal < min_signal_pixels {
        return Err(format!(
            "playfield-rect failed: playfield signal too low count={playfield_signal}, threshold={threshold:.6}"
        ));
    }

    println!(
        "playfield-rect ok: rect={}x{} at ({},{}), margins left={left_margin:.6} right={right_margin:.6}, bezel_signal=({},{}), playfield_signal={}",
        rect.width(),
        rect.height(),
        rect.left,
        rect.top,
        left_signal,
        right_signal,
        playfield_signal
    );
    Ok(())
}

fn cmd_audio_rms(args: &mut CliArgs) -> Result<(), String> {
    let wav = verify::load_wav(&args.path("--wav")?)?;
    let window_start_ms = args.value_or("--window-start-ms", 0.0_f32)?;
    let window_end_ms = args.value_or("--window-end-ms", f32::INFINITY)?;
    let min = args.value_or("--min", f32::NEG_INFINITY)?;
    let max = args.value_or("--max", f32::INFINITY)?;
    args.finish()?;
    let wav = windowed_wav(wav, window_start_ms, window_end_ms)?;
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
    let window_start_ms = args.value_or("--window-start-ms", 0.0_f32)?;
    let window_end_ms = args.value_or("--window-end-ms", f32::INFINITY)?;
    let min_hz = args.value_or("--min-hz", 0.0_f32)?;
    let max_hz = args.value_or("--max-hz", f32::INFINITY)?;
    args.finish()?;
    let wav = windowed_wav(wav, window_start_ms, window_end_ms)?;
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
    let window_start_ms = args.value_or("--window-start-ms", 0.0_f32)?;
    let window_end_ms = args.value_or("--window-end-ms", f32::INFINITY)?;
    let min_fraction = args.value_or("--min-fraction", 0.0_f32)?;
    args.finish()?;
    let wav = windowed_wav(wav, window_start_ms, window_end_ms)?;
    let fraction = verify::spectral_band_energy(&wav, lo_hz, hi_hz);
    if fraction < min_fraction {
        return Err(format!(
            "audio-band-energy failed: fraction={fraction:.6}, expected >= {min_fraction:.6}"
        ));
    }
    println!("audio-band-energy ok: fraction={fraction:.6}");
    Ok(())
}

fn windowed_wav(
    mut wav: verify::WavData,
    window_start_ms: f32,
    window_end_ms: f32,
) -> Result<verify::WavData, String> {
    if !window_start_ms.is_finite() || window_start_ms < 0.0 {
        return Err("--window-start-ms must be a finite non-negative number".to_string());
    }
    if window_end_ms < window_start_ms {
        return Err(
            "--window-end-ms must be greater than or equal to --window-start-ms".to_string(),
        );
    }

    let channels = usize::from(wav.channels.max(1));
    let total_frames = wav.samples.len() / channels;
    let start_frame = ((window_start_ms / 1000.0) * wav.sample_rate as f32)
        .floor()
        .clamp(0.0, total_frames as f32) as usize;
    let end_frame = if window_end_ms.is_finite() {
        ((window_end_ms / 1000.0) * wav.sample_rate as f32)
            .ceil()
            .clamp(0.0, total_frames as f32) as usize
    } else {
        total_frames
    };
    let start_sample = start_frame * channels;
    let end_sample = end_frame * channels;
    wav.samples = wav.samples[start_sample..end_sample].to_vec();
    Ok(wav)
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

fn cmd_heartbeat_tempo(args: &mut CliArgs) -> Result<(), String> {
    let wav = verify::load_wav(&args.path("--wav")?)?;
    let state_log = args.path("--state-log")?;
    let tempo_curve = args.value_or("--tempo-curve", "disassembly".to_string())?;
    let tolerance = args.value_or("--tolerance", 0.1_f32)?;
    args.finish()?;
    if tempo_curve != "disassembly" {
        return Err(format!(
            "heartbeat-tempo only supports --tempo-curve disassembly, got {tempo_curve}"
        ));
    }

    let state_events = verify::load_state_trace(&state_log)?;
    let state_points = heartbeat_state_points_from_events(&state_events);
    let max_count = state_points
        .iter()
        .map(|point| point.1)
        .max()
        .ok_or_else(|| "heartbeat-tempo failed: no asteroid_count states".to_string())?;
    let beats = detect_heartbeat_beats(&wav)?;
    if beats.len() < 4 {
        return Err(format!(
            "heartbeat-tempo failed: detected only {} beats",
            beats.len()
        ));
    }

    let mut checked = 0;
    let mut worst_error = 0.0_f32;
    for pair in beats.windows(2) {
        let t0 = pair[0];
        let t1 = pair[1];
        let Some(count0) = asteroid_count_at_time(&state_points, t0) else {
            continue;
        };
        let Some(count1) = asteroid_count_at_time(&state_points, t1) else {
            continue;
        };
        if count0 == 0 || count0 != count1 {
            continue;
        }
        let Some(expected) = audio::heartbeat_period_seconds_for_count(max_count, count0) else {
            continue;
        };
        let observed = t1 - t0;
        let error = (observed - expected).abs();
        worst_error = worst_error.max(error);
        if error > tolerance {
            return Err(format!(
                "heartbeat-tempo failed: count={count0} observed={observed:.3}s expected={expected:.3}s error={error:.3}s tolerance={tolerance:.3}s"
            ));
        }
        checked += 1;
    }

    if checked < 3 {
        return Err(format!(
            "heartbeat-tempo failed: only {checked} beat intervals matched stable state-log counts"
        ));
    }
    verify_heartbeat_death_continuity(&state_events, &beats)?;
    println!(
        "heartbeat-tempo ok: beats={} checked={} max_count={} worst_error={worst_error:.3}s",
        beats.len(),
        checked,
        max_count
    );
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

fn heartbeat_state_points_from_events(events: &[verify::StateTraceEvent]) -> Vec<(f32, u32)> {
    let mut points = Vec::new();
    for event in events {
        if event.event.as_deref() != Some("tick") {
            continue;
        }
        let Some(time) = event.value.get("time").and_then(Value::as_f64) else {
            continue;
        };
        let Some(count) = event.value.get("asteroid_count").and_then(Value::as_u64) else {
            continue;
        };
        points.push((time as f32, count as u32));
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    points
}

fn verify_heartbeat_death_continuity(
    events: &[verify::StateTraceEvent],
    beats: &[f32],
) -> Result<(), String> {
    let death_times = events
        .iter()
        .filter(|event| event.event.as_deref() == Some("ship-died"))
        .filter_map(|event| event.value.get("time").and_then(Value::as_f64))
        .map(|time| time as f32)
        .collect::<Vec<_>>();
    if death_times.len() < 2 {
        return Err(format!(
            "heartbeat-tempo failed: state log has only {} ship deaths; expected multiple deaths for continuity gate",
            death_times.len()
        ));
    }

    for death_time in death_times.iter().take(2).copied() {
        let has_followup_beat = beats
            .iter()
            .any(|beat| *beat >= death_time && *beat <= death_time + 1.5);
        if !has_followup_beat {
            return Err(format!(
                "heartbeat-tempo failed: no heartbeat detected within 1.5s after ship death at {death_time:.3}s"
            ));
        }
    }

    let Some(game_over_time) = events
        .iter()
        .find(|event| event.event.as_deref() == Some("game-over"))
        .and_then(|event| event.value.get("time").and_then(Value::as_f64))
        .map(|time| time as f32)
    else {
        return Ok(());
    };
    if beats.iter().any(|beat| *beat > game_over_time + 0.4) {
        return Err(format!(
            "heartbeat-tempo failed: heartbeat continued more than 0.4s after game-over at {game_over_time:.3}s"
        ));
    }
    Ok(())
}

fn asteroid_count_at_time(points: &[(f32, u32)], time: f32) -> Option<u32> {
    let mut current = None;
    for (point_time, count) in points {
        if *point_time > time {
            break;
        }
        current = Some(*count);
    }
    current
}

fn detect_heartbeat_beats(wav: &verify::WavData) -> Result<Vec<f32>, String> {
    let channels = usize::from(wav.channels.max(1));
    let frames = wav.samples.len() / channels;
    if frames == 0 {
        return Err("heartbeat-tempo failed: WAV has no samples".to_string());
    }
    let mono = wav.mono_samples(frames);
    let sample_rate = wav.sample_rate as usize;
    let window = (sample_rate / 100).max(1);
    let hop = (sample_rate / 200).max(1);
    let mut envelope = Vec::new();
    let mut frame = 0;
    while frame + window <= mono.len() {
        let rms = (mono[frame..frame + window]
            .iter()
            .map(|sample| sample * sample)
            .sum::<f32>()
            / window as f32)
            .sqrt();
        envelope.push((frame as f32 / wav.sample_rate as f32, rms));
        frame += hop;
    }
    let max_env = envelope
        .iter()
        .map(|(_, value)| *value)
        .fold(0.0_f32, f32::max);
    if max_env <= 0.0 {
        return Err("heartbeat-tempo failed: WAV envelope is silent".to_string());
    }
    let threshold = (max_env * 0.32).max(0.01);
    let mut beats = Vec::new();
    let mut last_beat_time = -1.0_f32;
    for i in 1..envelope.len().saturating_sub(1) {
        let (time, value) = envelope[i];
        if value < threshold {
            continue;
        }
        if value < envelope[i - 1].1 || value < envelope[i + 1].1 {
            continue;
        }
        if time - last_beat_time < 0.12 {
            continue;
        }
        beats.push(time);
        last_beat_time = time;
    }
    Ok(beats)
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
    run_highscore_helper_checks()?;
    println!(
        "state-trace ok: events={}, highscore_helpers=ok",
        events.len()
    );
    Ok(())
}

fn run_highscore_helper_checks() -> Result<(), String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let home = env::temp_dir().join(format!(
        "asteroids-verify-highscore-{}-{stamp}",
        process::id()
    ));
    let path = highscore::path_for_home(&home);

    let missing = highscore::read(&path)
        .map_err(|error| format!("highscore helper missing-file read failed: {error}"))?;
    if missing != 0 {
        let _ = fs::remove_dir_all(&home);
        return Err(format!(
            "highscore helper missing-file check failed: got {missing}, expected 0"
        ));
    }

    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| "highscore helper path has no parent".to_string())?,
    )
    .map_err(|error| format!("highscore helper failed to create temp dir: {error}"))?;
    fs::write(&path, "not an integer")
        .map_err(|error| format!("highscore helper failed to write corrupt file: {error}"))?;
    let corrupt = highscore::read(&path)
        .map_err(|error| format!("highscore helper corrupt-file read failed: {error}"))?;
    let _ = fs::remove_dir_all(&home);
    if corrupt != 0 {
        return Err(format!(
            "highscore helper corrupt-file check failed: got {corrupt}, expected 0"
        ));
    }

    Ok(())
}

fn cmd_asteroid_count(args: &mut CliArgs) -> Result<(), String> {
    let frames = args.path("--frames")?;
    let expected_large: i64 = args.value("--expected-large")?;
    let tolerance: i64 = args.value_or("--tolerance", 0_i64)?;
    args.finish()?;

    let metadata = load_frame_metadata(&frames)?;
    let sample = metadata
        .iter()
        .rev()
        .find(|value| {
            value
                .get("asteroids_large")
                .and_then(Value::as_i64)
                .is_some()
        })
        .ok_or_else(|| {
            format!(
                "asteroid-count failed: {} did not contain asteroid metadata",
                frames.display()
            )
        })?;
    let actual = sample
        .get("asteroids_large")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let delta = (actual - expected_large).abs();
    if delta > tolerance {
        return Err(format!(
            "asteroid-count failed: large={actual}, expected {expected_large} +/- {tolerance}"
        ));
    }
    println!("asteroid-count ok: large={actual}");
    Ok(())
}

fn cmd_screen_wrap(args: &mut CliArgs) -> Result<(), String> {
    let state_log = match args.optional_string("--state-log")? {
        Some(path) => PathBuf::from(path),
        None => args.path("--log")?,
    };
    args.finish()?;

    let events = verify::load_state_trace(&state_log)?;
    let mut tick_events = 0;
    let mut saw_asteroids = false;
    let mut saw_wrap = false;
    for event in &events {
        if event.event.as_deref() != Some("tick") {
            continue;
        }
        tick_events += 1;
        if event
            .value
            .get("asteroid_wrapped")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            saw_wrap = true;
        }
        let Some(asteroids) = event.value.get("asteroids").and_then(Value::as_array) else {
            continue;
        };
        if !asteroids.is_empty() {
            saw_asteroids = true;
        }
        for asteroid in asteroids {
            let x = asteroid.get("x").and_then(Value::as_f64).ok_or_else(|| {
                format!(
                    "screen-wrap failed: missing asteroid x at line {}",
                    event.line
                )
            })?;
            let y = asteroid.get("y").and_then(Value::as_f64).ok_or_else(|| {
                format!(
                    "screen-wrap failed: missing asteroid y at line {}",
                    event.line
                )
            })?;
            if !(-1.000001..=1.000001).contains(&x) || !(-1.000001..=1.000001).contains(&y) {
                return Err(format!(
                    "screen-wrap failed: asteroid out of playfield at line {}: x={x:.6} y={y:.6}",
                    event.line
                ));
            }
            if asteroid
                .get("wrapped")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                saw_wrap = true;
            }
        }
    }
    if tick_events == 0 {
        return Err("screen-wrap failed: no tick events in state log".to_string());
    }
    if !saw_asteroids {
        return Err("screen-wrap failed: no asteroid state in tick events".to_string());
    }
    if !saw_wrap {
        return Err("screen-wrap failed: no asteroid wrap event observed".to_string());
    }
    println!("screen-wrap ok: tick_events={tick_events}");
    Ok(())
}

fn cmd_lives_display(args: &mut CliArgs) -> Result<(), String> {
    let image = verify::load_png(&args.path("--frame")?)?;
    let max_displayed = args.value("--max-displayed")?;
    let state_log = args.path("--state-log")?;
    args.finish()?;
    if max_displayed == 0 {
        return Err("--max-displayed must be greater than zero".to_string());
    }

    let state_lives = max_lives_in_state_log(&state_log)?;
    if state_lives < max_displayed {
        return Err(format!(
            "lives-display failed: state log max lives={state_lives}, expected at least {max_displayed}"
        ));
    }

    let scores = life_icon_scores(&image, max_displayed.saturating_add(2))?;
    let present_scores = &scores[..max_displayed.min(scores.len())];
    let min_present = present_scores.iter().copied().fold(f32::INFINITY, f32::min);
    let present_threshold = 0.025;
    if min_present < present_threshold {
        return Err(format!(
            "lives-display failed: weakest expected icon score={min_present:.6}, threshold={present_threshold:.6}, scores={scores:?}"
        ));
    }

    let absent_limit = (min_present * 0.35).max(0.02);
    let extra_visible = scores
        .iter()
        .copied()
        .enumerate()
        .skip(max_displayed)
        .filter(|(_, score)| *score >= absent_limit)
        .collect::<Vec<_>>();
    if !extra_visible.is_empty() {
        return Err(format!(
            "lives-display failed: icons beyond cap are visible at {extra_visible:?}, absent_limit={absent_limit:.6}, scores={scores:?}"
        ));
    }

    println!(
        "lives-display ok: state_lives={state_lives}, displayed_cap={max_displayed}, weakest_icon={min_present:.6}"
    );
    Ok(())
}

fn max_lives_in_state_log(path: &Path) -> Result<usize, String> {
    let events = verify::load_state_trace(path)?;
    let mut best = None;
    for event in events {
        if event.event.as_deref() != Some("tick") {
            continue;
        }
        let Some(lives) = event.value.get("lives").and_then(Value::as_u64) else {
            continue;
        };
        best = Some(best.unwrap_or(0).max(lives as usize));
    }
    best.ok_or_else(|| {
        format!(
            "lives-display failed: no tick events with lives in {}",
            path.display()
        )
    })
}

fn life_icon_scores(image: &verify::PngImage, count: usize) -> Result<Vec<f32>, String> {
    let playfield = centered_aspect_rect(image.width, image.height, PLAYFIELD_ASPECT_RATIO)?;
    if playfield.right >= image.width {
        return Err("lives-display failed: image has no right bezel margin".to_string());
    }

    let margin_width_ndc =
        image.width.saturating_sub(playfield.right) as f32 / image.width.max(1) as f32 * 2.0;
    let icon_scale = (margin_width_ndc * 0.62 / 0.44).clamp(0.055, 0.16);
    let center_x_px = (playfield.right + image.width) as f32 * 0.5;
    let center_x_ndc = center_x_px / image.width.max(1) as f32 * 2.0 - 1.0;

    Ok((0..count)
        .map(|index| {
            life_icon_score(
                image,
                asteroids::beam::Vec2::new(center_x_ndc, 0.82 - index as f32 * 0.14),
                icon_scale,
            )
        })
        .collect())
}

fn life_icon_score(image: &verify::PngImage, center: asteroids::beam::Vec2, scale: f32) -> f32 {
    let angle = PI * 0.5;
    let mut score = 0.0;
    let mut samples = 0;
    for (start, end) in SHIP_SEGMENTS {
        let local_start = SHIP_VERTICES[start] * scale;
        let local_end = SHIP_VERTICES[end] * scale;
        for sample in [0.25_f32, 0.5, 0.75] {
            let local = local_start + (local_end - local_start) * sample;
            let point = frame_ndc_to_pixel(image, center + rotate_vec2(local, angle));
            score += max_signal_near(image, point, 4);
            samples += 1;
        }
    }
    score / samples.max(1) as f32
}

fn frame_ndc_to_pixel(image: &verify::PngImage, point: asteroids::beam::Vec2) -> (f32, f32) {
    (
        (point.x * 0.5 + 0.5) * image.width.saturating_sub(1) as f32,
        (0.5 - point.y * 0.5) * image.height.saturating_sub(1) as f32,
    )
}

fn load_frame_metadata(frames: &Path) -> Result<Vec<Value>, String> {
    let path = frames.join(asteroids::runtime::FRAME_METADATA_FILENAME);
    let content = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read frame metadata {}: {error}", path.display()))?;
    let mut values = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed).map_err(|error| {
            format!(
                "failed to parse frame metadata {} line {}: {error}",
                path.display(),
                line_index + 1
            )
        })?;
        values.push(value);
    }
    if values.is_empty() {
        return Err(format!("frame metadata {} had no samples", path.display()));
    }
    Ok(values)
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
    let playfield = centered_aspect_rect(first.width, first.height, PLAYFIELD_ASPECT_RATIO)?;
    let (peak_x, peak_y, peak_luma) = brightest_pixel_in_rect(&first, playfield);
    if peak_luma <= 0.0 {
        return Err(format!("first frame in {} has no luminance", dir.display()));
    }

    let mut coords = Vec::with_capacity(playfield.width() as usize * 5 + 5);
    for y_offset in -2_i32..=2 {
        let y = (peak_y as i32 + y_offset).clamp(
            playfield.top as i32,
            playfield.bottom.saturating_sub(1) as i32,
        ) as u32;
        for x in playfield.left..playfield.right {
            coords.push((x, y));
        }
    }
    for y in playfield.top..playfield.bottom {
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

fn brightest_pixel_in_rect(image: &verify::PngImage, rect: PixelRect) -> (u32, u32, f32) {
    let mut best = (0, 0, 0.0);
    let left = rect.left.min(image.width);
    let right = rect.right.min(image.width);
    let top = rect.top.min(image.height);
    let bottom = rect.bottom.min(image.height);
    for y in top..bottom {
        for x in left..right {
            let luma = image.luminance(x, y);
            if luma > best.2 {
                best = (x, y, luma);
            }
        }
    }
    best
}

fn brightest_pixel(image: &verify::PngImage) -> (u32, u32, f32) {
    brightest_pixel_in_rect(
        image,
        PixelRect {
            left: 0,
            right: image.width,
            top: 0,
            bottom: image.height,
        },
    )
}

const VERIFY_FIXED_DT_SECONDS: f32 = 0.00694;
const PLAYFIELD_ASPECT_RATIO: f32 = 4.0 / 3.0;
const SOUL_VISIBLE_MIN_FRAMES: usize = 8;
const SOUL_VISIBLE_ROTATION_TOLERANCE_RAD_PER_SEC: f32 = 0.1;
const SOUL_VISIBLE_MIN_TRAIL_LUMINANCE: f32 = 0.0;
const SHIP_VERTEX_COUNT: usize = 4;
const SHIP_VERTICES: [asteroids::beam::Vec2; SHIP_VERTEX_COUNT] = [
    asteroids::beam::Vec2::new(0.44, 0.0),
    asteroids::beam::Vec2::new(-0.30, 0.22),
    asteroids::beam::Vec2::new(-0.12, 0.0),
    asteroids::beam::Vec2::new(-0.30, -0.22),
];
const SHIP_SEGMENTS: [(usize, usize); SHIP_VERTEX_COUNT] = [(0, 1), (1, 2), (2, 3), (3, 0)];
const SHIP_ANGLE_SEARCH_RADIUS_RAD: f32 = 0.25;
const SHIP_ANGLE_COARSE_STEP_RAD: f32 = 0.006;
const SHIP_ANGLE_FINE_RADIUS_RAD: f32 = 0.012;
const SHIP_ANGLE_FINE_STEP_RAD: f32 = 0.001;

#[derive(Clone, Debug)]
struct GammaRampSample {
    index: usize,
    input: f32,
    actual: f32,
    expected: f32,
}

#[derive(Clone, Debug)]
struct ShipDetection {
    angle_rad: f32,
    vertex_luminance: [f32; SHIP_VERTEX_COUNT],
}

#[derive(Clone, Copy, Debug)]
struct AngleRateFit {
    rate_rad_per_sec: f32,
    max_residual_rad: f32,
}

fn detect_ship_angles(
    frames: &[PathBuf],
    rotation_rate_rad_per_sec: f32,
) -> Result<Vec<ShipDetection>, String> {
    let mut detections = Vec::with_capacity(frames.len());
    let expected_delta = rotation_rate_rad_per_sec * VERIFY_FIXED_DT_SECONDS;
    let mut initial_angle = None;

    for (frame_index, path) in frames.iter().enumerate() {
        let image = verify::load_png(path)?;
        let playfield = centered_aspect_rect(image.width, image.height, PLAYFIELD_ASPECT_RATIO)?;

        let angle = if let Some(initial_angle) = initial_angle {
            let predicted_angle = initial_angle + expected_delta * frame_index as f32;
            measure_ship_angle(
                &image,
                playfield,
                predicted_angle,
                SHIP_ANGLE_SEARCH_RADIUS_RAD,
            )
        } else {
            let angle = measure_ship_angle(&image, playfield, PI, PI);
            initial_angle = Some(angle);
            angle
        };
        let vertex_luminance = ship_vertex_luminance(&image, playfield, angle);
        let weakest_vertex = vertex_luminance.into_iter().fold(f32::INFINITY, f32::min);
        let vertex_threshold = 0.025;
        if weakest_vertex < vertex_threshold {
            return Err(format!(
                "ship-outline failed: frame {frame_index} detected {SHIP_VERTEX_COUNT} vertices but weakest luma={weakest_vertex:.6} below threshold={vertex_threshold:.6}"
            ));
        }

        detections.push(ShipDetection {
            angle_rad: angle,
            vertex_luminance,
        });
    }

    Ok(detections)
}

fn measure_ship_angle(
    image: &verify::PngImage,
    playfield: PixelRect,
    center: f32,
    radius: f32,
) -> f32 {
    let coarse = best_ship_angle(image, playfield, center, radius, SHIP_ANGLE_COARSE_STEP_RAD);
    best_ship_angle(
        image,
        playfield,
        coarse,
        SHIP_ANGLE_FINE_RADIUS_RAD,
        SHIP_ANGLE_FINE_STEP_RAD,
    )
}

fn best_ship_angle(
    image: &verify::PngImage,
    playfield: PixelRect,
    center: f32,
    radius: f32,
    step: f32,
) -> f32 {
    let steps = ((radius * 2.0) / step).ceil().max(1.0) as i32;
    let start = center - radius;
    let mut best_angle = center;
    let mut best_score = f32::NEG_INFINITY;
    for i in 0..=steps {
        let angle = start + i as f32 * step;
        let score = ship_outline_score(image, playfield, angle);
        if score > best_score {
            best_score = score;
            best_angle = angle;
        }
    }
    best_angle
}

fn ship_outline_score(image: &verify::PngImage, playfield: PixelRect, angle: f32) -> f32 {
    let vertices = ship_pixels(playfield, angle);
    let mut score = 0.0;
    let mut samples = 0;
    for (start, end) in SHIP_SEGMENTS {
        let a = vertices[start];
        let b = vertices[end];
        for i in [3, 5, 7, 9, 11, 13] {
            let t = i as f32 / 16.0;
            let point = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            score += max_signal_near(image, point, 1);
            samples += 1;
        }
    }
    score / samples.max(1) as f32
}

fn ship_vertex_luminance(
    image: &verify::PngImage,
    playfield: PixelRect,
    angle: f32,
) -> [f32; SHIP_VERTEX_COUNT] {
    ship_pixels(playfield, angle).map(|point| max_signal_near(image, point, 4))
}

fn ship_pixels(playfield: PixelRect, angle: f32) -> [(f32, f32); SHIP_VERTEX_COUNT] {
    SHIP_VERTICES.map(|vertex| {
        gameplay_to_pixel(
            playfield,
            rotate_vec2(vertex * asteroids::tuning::SHIP_SPINNING_SCALE, angle),
        )
    })
}

fn gameplay_to_pixel(playfield: PixelRect, point: asteroids::beam::Vec2) -> (f32, f32) {
    (
        playfield.left as f32 + (point.x * 0.5 + 0.5) * playfield.width() as f32,
        playfield.top as f32 + (0.5 - point.y * 0.5) * playfield.height() as f32,
    )
}

fn gamma_ramp_samples(
    image: &verify::PngImage,
    playfield: PixelRect,
    gamma: f32,
) -> Vec<GammaRampSample> {
    let mut samples = Vec::with_capacity(asteroids::tuning::GAMMA_RAMP_BARS);
    let center_x =
        (asteroids::tuning::GAMMA_RAMP_X_MIN + asteroids::tuning::GAMMA_RAMP_X_MAX) * 0.5;
    for index in 0..asteroids::tuning::GAMMA_RAMP_BARS {
        let input = index as f32 / (asteroids::tuning::GAMMA_RAMP_BARS - 1) as f32;
        let y = asteroids::tuning::GAMMA_RAMP_Y_MIN
            + input * (asteroids::tuning::GAMMA_RAMP_Y_MAX - asteroids::tuning::GAMMA_RAMP_Y_MIN);
        let point = gameplay_to_pixel(playfield, asteroids::beam::Vec2::new(center_x, y));
        let actual = max_signal_near(image, point, 4);
        let tone_mapped = input / (input + 1.0);
        let expected = tone_mapped.clamp(0.0, 1.0).powf(1.0 / gamma);
        samples.push(GammaRampSample {
            index,
            input,
            actual,
            expected,
        });
    }
    samples
}

fn rotate_vec2(point: asteroids::beam::Vec2, angle: f32) -> asteroids::beam::Vec2 {
    let (sin, cos) = angle.sin_cos();
    asteroids::beam::Vec2::new(point.x * cos - point.y * sin, point.x * sin + point.y * cos)
}

fn max_luminance_near(image: &verify::PngImage, point: (f32, f32), radius: i32) -> f32 {
    let x = point.0.round() as i32;
    let y = point.1.round() as i32;
    let mut best = 0.0;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let px = (x + dx).clamp(0, image.width.saturating_sub(1) as i32) as u32;
            let py = (y + dy).clamp(0, image.height.saturating_sub(1) as i32) as u32;
            best = f32::max(best, image.luminance(px, py));
        }
    }
    best
}

fn max_signal_near(image: &verify::PngImage, point: (f32, f32), radius: i32) -> f32 {
    let x = point.0.round() as i32;
    let y = point.1.round() as i32;
    let mut best = 0.0;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let px = (x + dx).clamp(0, image.width.saturating_sub(1) as i32) as u32;
            let py = (y + dy).clamp(0, image.height.saturating_sub(1) as i32) as u32;
            let [red, green, blue, _] = image.pixel_rgba(px, py);
            best = f32::max(best, red.max(green).max(blue) as f32 / 255.0);
        }
    }
    best
}

fn fit_angle_rate(detections: &[ShipDetection], fixed_dt: f32) -> AngleRateFit {
    let n = detections.len() as f32;
    let mean_time = fixed_dt * (n - 1.0) * 0.5;
    let mean_angle = detections
        .iter()
        .map(|detection| detection.angle_rad)
        .sum::<f32>()
        / n;
    let mut ss_xy = 0.0;
    let mut ss_xx = 0.0;
    for (index, detection) in detections.iter().enumerate() {
        let dt = index as f32 * fixed_dt - mean_time;
        ss_xy += dt * (detection.angle_rad - mean_angle);
        ss_xx += dt * dt;
    }
    let rate = if ss_xx > f32::EPSILON {
        ss_xy / ss_xx
    } else {
        0.0
    };
    let intercept = mean_angle - rate * mean_time;
    let max_residual = detections
        .iter()
        .enumerate()
        .map(|(index, detection)| {
            (detection.angle_rad - (intercept + rate * index as f32 * fixed_dt)).abs()
        })
        .fold(0.0, f32::max);
    AngleRateFit {
        rate_rad_per_sec: rate,
        max_residual_rad: max_residual,
    }
}

fn estimated_fwhm_width(image: &verify::PngImage) -> f32 {
    let playfield = centered_aspect_rect(image.width, image.height, PLAYFIELD_ASPECT_RATIO)
        .unwrap_or(PixelRect {
            left: 0,
            right: image.width,
            top: 0,
            bottom: image.height,
        });
    let best = brightest_pixel_in_rect(image, playfield);
    let row_values = (playfield.left..playfield.right)
        .map(|x| image.luminance(x, best.1))
        .collect::<Vec<_>>();
    let col_values = (playfield.top..playfield.bottom)
        .map(|y| image.luminance(best.0, y))
        .collect::<Vec<_>>();
    let row_width = fwhm_width_1d(&row_values, best.0.saturating_sub(playfield.left) as usize);
    let col_width = fwhm_width_1d(&col_values, best.1.saturating_sub(playfield.top) as usize);
    if row_width <= 0.0 {
        col_width
    } else if col_width <= 0.0 {
        row_width
    } else {
        row_width.min(col_width)
    }
}

fn banding_luminance_values(image: &verify::PngImage) -> Vec<f32> {
    let (_, peak_y, peak_luma) = brightest_pixel(image);
    let x_range = centered_aspect_rect(image.width, image.height, PLAYFIELD_ASPECT_RATIO)
        .map(|rect| (rect.left, rect.right.saturating_sub(1)))
        .unwrap_or((0, image.width.saturating_sub(1)));
    if peak_luma <= 0.0 {
        return horizontal_luminance_values(image, image.height / 2, x_range);
    }

    let min_signal = (peak_luma * 0.02).max(0.003);
    let target_signal = peak_luma * 0.25;
    let max_signal = peak_luma * 0.55;
    let max_offset = (image.height / 3).clamp(1, 512) as i32;
    let mut best: Option<(f32, i32, Vec<f32>)> = None;

    for offset in 1..=max_offset {
        for sign in [1_i32, -1_i32] {
            let y = peak_y as i32 + offset * sign;
            if y < 0 || y >= image.height as i32 {
                continue;
            }

            let values = horizontal_luminance_values(image, y as u32, x_range);
            let row_max = values.iter().copied().fold(0.0_f32, f32::max);
            if row_max < min_signal || row_max > max_signal {
                continue;
            }

            let score = (row_max - target_signal).abs();
            if best.as_ref().is_none_or(|(best_score, best_offset, _)| {
                score < *best_score || (score == *best_score && offset < best_offset.abs())
            }) {
                best = Some((score, offset * sign, values));
            }
        }
    }

    best.map(|(_, _, values)| values)
        .unwrap_or_else(|| horizontal_luminance_values(image, image.height / 2, x_range))
}

fn horizontal_luminance_values(image: &verify::PngImage, y: u32, x_range: (u32, u32)) -> Vec<f32> {
    let y = y.min(image.height.saturating_sub(1)) as f32;
    let start_x = x_range.0.min(image.width.saturating_sub(1));
    let end_x = x_range.1.min(image.width.saturating_sub(1)).max(start_x);
    verify::line_luminance(
        image,
        (start_x as f32, y),
        (end_x as f32, y),
        (end_x - start_x + 1).min(1024) as usize,
    )
}

fn fwhm_width_1d(values: &[f32], peak_index: usize) -> f32 {
    if values.is_empty() || peak_index >= values.len() {
        return 0.0;
    }
    let peak = values[peak_index];
    if peak <= 0.0 {
        return 0.0;
    }
    let threshold = peak * 0.5;

    let mut left_index = peak_index;
    while left_index > 0 && values[left_index] >= threshold {
        left_index -= 1;
    }
    let left = if values[left_index] >= threshold {
        0.0
    } else {
        half_crossing(
            values[left_index],
            values[left_index + 1],
            threshold,
            left_index,
        )
    };

    let mut right_index = peak_index;
    while right_index + 1 < values.len() && values[right_index + 1] >= threshold {
        right_index += 1;
    }
    let right = if right_index + 1 == values.len() {
        (values.len() - 1) as f32
    } else {
        half_crossing(
            values[right_index],
            values[right_index + 1],
            threshold,
            right_index,
        )
    };

    (right - left).max(0.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelRect {
    left: u32,
    right: u32,
    top: u32,
    bottom: u32,
}

impl PixelRect {
    fn width(self) -> u32 {
        self.right.saturating_sub(self.left)
    }

    fn height(self) -> u32 {
        self.bottom.saturating_sub(self.top)
    }
}

fn parse_aspect_ratio(value: &str) -> Result<f32, String> {
    let (width, height) = value
        .split_once(':')
        .ok_or_else(|| format!("--aspect must use <width>:<height>, got '{value}'"))?;
    let width: f32 = width
        .trim()
        .parse()
        .map_err(|error| format!("--aspect width is invalid: {error}"))?;
    let height: f32 = height
        .trim()
        .parse()
        .map_err(|error| format!("--aspect height is invalid: {error}"))?;
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err(format!(
            "--aspect must be positive and finite, got '{value}'"
        ));
    }
    Ok(width / height)
}

fn centered_aspect_rect(width: u32, height: u32, aspect: f32) -> Result<PixelRect, String> {
    if width == 0 || height == 0 {
        return Err("playfield-rect failed: image dimensions must be non-zero".to_string());
    }
    if !aspect.is_finite() || aspect <= 0.0 {
        return Err(format!("invalid aspect ratio {aspect}"));
    }

    let image_aspect = width as f32 / height as f32;
    let (rect_width, rect_height) = if image_aspect >= aspect {
        (
            ((height as f32 * aspect).round() as u32).clamp(1, width),
            height,
        )
    } else {
        (
            width,
            ((width as f32 / aspect).round() as u32).clamp(1, height),
        )
    };
    let left = (width - rect_width) / 2;
    let top = (height - rect_height) / 2;

    Ok(PixelRect {
        left,
        right: left + rect_width,
        top,
        bottom: top + rect_height,
    })
}

fn signal_count(
    image: &verify::PngImage,
    x_start: u32,
    x_end: u32,
    y_start: u32,
    y_end: u32,
    threshold: f32,
) -> usize {
    let x_start = x_start.min(image.width);
    let x_end = x_end.min(image.width).max(x_start);
    let y_start = y_start.min(image.height);
    let y_end = y_end.min(image.height).max(y_start);
    let mut count = 0;
    for y in y_start..y_end {
        for x in x_start..x_end {
            if image.luminance(x, y) >= threshold {
                count += 1;
            }
        }
    }
    count
}

fn half_crossing(a: f32, b: f32, threshold: f32, left_index: usize) -> f32 {
    let denom = b - a;
    if denom.abs() <= f32::EPSILON {
        left_index as f32
    } else {
        left_index as f32 + ((threshold - a) / denom).clamp(0.0, 1.0)
    }
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
        "gamma-ramp" => "Usage: verify gamma-ramp --frame <png> [--gamma <value>] [--tolerance <value>] [--min-steps <n>]".to_string(),
        "banding" => "Usage: verify banding --frame <png> --max-step-jump <luma>".to_string(),
        "peak-count" => "Usage: verify peak-count --frame <png> [--threshold <luma>] [--min <n>] [--max <n>]".to_string(),
        "ship-outline" => "Usage: verify ship-outline --frames <dir> --vertex-count <n> --rotation-rate-rad-per-sec <rate> --tolerance <value>".to_string(),
        "trail-luminance" => "Usage: verify trail-luminance --frames <dir> --behind-vector --min-luminance <value>".to_string(),
        "soul-visible" => "Usage: verify soul-visible --frames <dir> [--rotation-tolerance <rad/s>] [--min-trail-luminance <luma>]".to_string(),
        "asteroid-count" => "Usage: verify asteroid-count --frames <dir> --expected-large <n> [--tolerance <n>]".to_string(),
        "screen-wrap" => "Usage: verify screen-wrap --state-log <state-jsonl>".to_string(),
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

    fn flag(&mut self, flag: &str) -> bool {
        let Some(index) = self.args.iter().position(|arg| arg == flag) else {
            return false;
        };
        self.args.remove(index);
        true
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

#[cfg(test)]
mod tests {
    use super::fwhm_width_1d;

    #[test]
    fn fwhm_width_1d_interpolates_narrow_peak() {
        let values = [0.0, 0.25, 1.0, 0.25, 0.0];
        let width = fwhm_width_1d(&values, 2);
        assert!((width - 4.0 / 3.0).abs() < 0.0001, "width={width}");
    }

    #[test]
    fn fwhm_width_1d_handles_peak_at_left_edge() {
        let values = [1.0, 0.25, 0.0];
        let width = fwhm_width_1d(&values, 0);
        assert!((width - 2.0 / 3.0).abs() < 0.0001, "width={width}");
    }

    #[test]
    fn fwhm_width_1d_handles_peak_at_right_edge() {
        let values = [0.0, 0.25, 1.0];
        let width = fwhm_width_1d(&values, 2);
        assert!((width - 2.0 / 3.0).abs() < 0.0001, "width={width}");
    }
}
