use std::{
    env,
    f32::consts::PI,
    fs,
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
        "soul-visible" | "asteroid-count" | "screen-wrap" | "lives-display" | "heartbeat-tempo" => {
            Err(format!(
                "{command} is reserved for a later gameplay/audio milestone\n\n{}",
                subcommand_help(&command)
            ))
        }
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

    if best_luminance < min_luminance {
        return Err(format!(
            "trail-luminance failed: behind-vector luminance={best_luminance:.6}, expected >= {min_luminance:.6}"
        ));
    }
    println!("trail-luminance ok: behind-vector luminance={best_luminance:.6}");
    Ok(())
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

const VERIFY_FIXED_DT_SECONDS: f32 = 0.00694;
const PLAYFIELD_ASPECT_RATIO: f32 = 4.0 / 3.0;
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
    let best = brightest_pixel(image);
    let row_values = (0..image.width)
        .map(|x| image.luminance(x, best.1))
        .collect::<Vec<_>>();
    let col_values = (0..image.height)
        .map(|y| image.luminance(best.0, y))
        .collect::<Vec<_>>();
    let row_width = fwhm_width_1d(&row_values, best.0 as usize);
    let col_width = fwhm_width_1d(&col_values, best.1 as usize);
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
    if peak_luma <= 0.0 {
        return horizontal_luminance_values(image, image.height / 2);
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

            let values = horizontal_luminance_values(image, y as u32);
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
        .unwrap_or_else(|| horizontal_luminance_values(image, image.height / 2))
}

fn horizontal_luminance_values(image: &verify::PngImage, y: u32) -> Vec<f32> {
    let y = y.min(image.height.saturating_sub(1)) as f32;
    verify::line_luminance(
        image,
        (0.0, y),
        (image.width.saturating_sub(1) as f32, y),
        image.width.min(1024) as usize,
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
        "gamma-ramp" => "Usage: verify gamma-ramp --frame <png> [--min-steps <n>]".to_string(),
        "banding" => "Usage: verify banding --frame <png> --max-step-jump <luma>".to_string(),
        "peak-count" => "Usage: verify peak-count --frame <png> [--threshold <luma>] [--min <n>] [--max <n>]".to_string(),
        "ship-outline" => "Usage: verify ship-outline --frames <dir> --vertex-count <n> --rotation-rate-rad-per-sec <rate> --tolerance <value>".to_string(),
        "trail-luminance" => "Usage: verify trail-luminance --frames <dir> --behind-vector --min-luminance <value>".to_string(),
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
