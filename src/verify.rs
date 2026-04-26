use std::{
    f32::consts::TAU,
    fs::{self, File},
    io::{BufReader, Read},
    path::Path,
};

use serde_json::Value;

#[derive(Clone, Debug)]
pub struct PngImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl PngImage {
    pub fn pixel_rgba(&self, x: u32, y: u32) -> [u8; 4] {
        let x = x.min(self.width.saturating_sub(1));
        let y = y.min(self.height.saturating_sub(1));
        let offset = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[offset],
            self.rgba[offset + 1],
            self.rgba[offset + 2],
            self.rgba[offset + 3],
        ]
    }

    pub fn luminance(&self, x: u32, y: u32) -> f32 {
        hdr_luminance_from_rgba(self.pixel_rgba(x, y))
    }

    pub fn max_luminance(&self) -> f32 {
        self.rgba
            .chunks_exact(4)
            .map(|px| hdr_luminance_from_rgba([px[0], px[1], px[2], px[3]]))
            .fold(0.0, f32::max)
    }
}

pub fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    if rgba.len() != (width * height * 4) as usize {
        return Err(format!(
            "rgba buffer has {} bytes, expected {}",
            rgba.len(),
            width * height * 4
        ));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }

    let file = File::create(path)
        .map_err(|error| format!("failed to create PNG {}: {error}", path.display()))?;
    let writer = std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| format!("failed to write PNG header {}: {error}", path.display()))?;
    writer
        .write_image_data(rgba)
        .map_err(|error| format!("failed to write PNG data {}: {error}", path.display()))
}

pub fn load_png(path: &Path) -> Result<PngImage, String> {
    let file = File::open(path)
        .map_err(|error| format!("failed to open PNG {}: {error}", path.display()))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("failed to read PNG info {}: {error}", path.display()))?;
    let output_buffer_size = reader
        .output_buffer_size()
        .ok_or_else(|| format!("PNG output buffer would overflow usize: {}", path.display()))?;
    let mut bytes = vec![0; output_buffer_size];
    let info = reader
        .next_frame(&mut bytes)
        .map_err(|error| format!("failed to decode PNG {}: {error}", path.display()))?;
    let frame = &bytes[..info.buffer_size()];
    let mut rgba = Vec::with_capacity((info.width * info.height * 4) as usize);

    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(frame),
        png::ColorType::Rgb => {
            for px in frame.chunks_exact(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for luma in frame {
                rgba.extend_from_slice(&[*luma, *luma, *luma, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for px in frame.chunks_exact(2) {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        png::ColorType::Indexed => {
            return Err(format!(
                "indexed PNG {} was not expanded by the decoder",
                path.display()
            ));
        }
    }

    Ok(PngImage {
        width: info.width,
        height: info.height,
        rgba,
    })
}

pub fn line_luminance(
    image: &PngImage,
    start: (f32, f32),
    end: (f32, f32),
    samples: usize,
) -> Vec<f32> {
    let samples = samples.max(2);
    let mut values = Vec::with_capacity(samples);
    for i in 0..samples {
        let t = i as f32 / (samples - 1) as f32;
        let x = start.0 + (end.0 - start.0) * t;
        let y = start.1 + (end.1 - start.1) * t;
        values.push(sample_luminance(image, x, y));
    }
    values
}

pub fn peak_count(values: &[f32], threshold: f32) -> usize {
    if values.len() < 3 {
        return values.iter().filter(|value| **value >= threshold).count();
    }
    values
        .windows(3)
        .filter(|window| window[1] >= threshold && window[1] >= window[0] && window[1] >= window[2])
        .count()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecayFit {
    pub tau_seconds: f64,
    pub r_squared: f64,
}

pub fn decay_fit(samples: &[(f64, f64)]) -> Option<DecayFit> {
    let log_samples: Vec<(f64, f64)> = samples
        .iter()
        .copied()
        .filter(|(_, value)| value.is_finite() && *value > 0.0)
        .map(|(time, value)| (time, value.ln()))
        .collect();
    if log_samples.len() < 3 {
        return None;
    }

    let n = log_samples.len() as f64;
    let mean_x = log_samples.iter().map(|(x, _)| *x).sum::<f64>() / n;
    let mean_y = log_samples.iter().map(|(_, y)| *y).sum::<f64>() / n;
    let mut ss_xy = 0.0;
    let mut ss_xx = 0.0;
    let mut ss_tot = 0.0;
    for (x, y) in &log_samples {
        let dx = x - mean_x;
        let dy = y - mean_y;
        ss_xy += dx * dy;
        ss_xx += dx * dx;
        ss_tot += dy * dy;
    }
    if ss_xx <= f64::EPSILON {
        return None;
    }

    let slope = ss_xy / ss_xx;
    if slope >= 0.0 {
        return None;
    }
    let intercept = mean_y - slope * mean_x;
    let ss_res = log_samples
        .iter()
        .map(|(x, y)| {
            let residual = y - (slope * x + intercept);
            residual * residual
        })
        .sum::<f64>();
    let r_squared = if ss_tot <= f64::EPSILON {
        1.0
    } else {
        1.0 - ss_res / ss_tot
    };
    Some(DecayFit {
        tau_seconds: -1.0 / slope,
        r_squared,
    })
}

#[derive(Clone, Debug)]
pub struct WavData {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl WavData {
    pub fn mono_samples(&self, max_samples: usize) -> Vec<f32> {
        let frames = self.samples.len() / usize::from(self.channels.max(1));
        let frames = frames.min(max_samples);
        let mut mono = Vec::with_capacity(frames);
        for frame in 0..frames {
            let offset = frame * usize::from(self.channels);
            let sum = self.samples[offset..offset + usize::from(self.channels)]
                .iter()
                .sum::<f32>();
            mono.push(sum / f32::from(self.channels));
        }
        mono
    }
}

pub fn load_wav(path: &Path) -> Result<WavData, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| format!("failed to open WAV {}: {error}", path.display()))?
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read WAV {}: {error}", path.display()))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("{} is not a RIFF/WAVE file", path.display()));
    }

    let mut offset = 12;
    let mut format = None;
    let mut channels = None;
    let mut sample_rate = None;
    let mut bits_per_sample = None;
    let mut data = None;

    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_len = read_u32_le(&bytes[offset + 4..offset + 8]) as usize;
        offset += 8;
        if offset + chunk_len > bytes.len() {
            return Err(format!("WAV chunk overruns file: {}", path.display()));
        }
        match chunk_id {
            b"fmt " if chunk_len >= 16 => {
                format = Some(read_u16_le(&bytes[offset..offset + 2]));
                channels = Some(read_u16_le(&bytes[offset + 2..offset + 4]));
                sample_rate = Some(read_u32_le(&bytes[offset + 4..offset + 8]));
                bits_per_sample = Some(read_u16_le(&bytes[offset + 14..offset + 16]));
            }
            b"data" => data = Some(offset..offset + chunk_len),
            _ => {}
        }
        offset += chunk_len + (chunk_len % 2);
    }

    let format = format.ok_or_else(|| format!("WAV fmt chunk missing: {}", path.display()))?;
    let channels = channels.ok_or_else(|| format!("WAV channels missing: {}", path.display()))?;
    let sample_rate =
        sample_rate.ok_or_else(|| format!("WAV sample rate missing: {}", path.display()))?;
    let bits_per_sample =
        bits_per_sample.ok_or_else(|| format!("WAV bit depth missing: {}", path.display()))?;
    let data = data.ok_or_else(|| format!("WAV data chunk missing: {}", path.display()))?;

    let samples = match (format, bits_per_sample) {
        (1, 16) => bytes[data]
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0)
            .collect(),
        (1, 8) => bytes[data]
            .iter()
            .map(|sample| (*sample as f32 - 128.0) / 128.0)
            .collect(),
        (3, 32) => bytes[data]
            .chunks_exact(4)
            .map(|sample| f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]))
            .collect(),
        _ => {
            return Err(format!(
                "unsupported WAV format={}, bits={} in {}",
                format,
                bits_per_sample,
                path.display()
            ));
        }
    };

    Ok(WavData {
        sample_rate,
        channels,
        samples,
    })
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

pub fn dominant_freq(wav: &WavData) -> Option<f32> {
    let mono = prepare_fft_window(wav);
    if mono.len() < 8 {
        return None;
    }
    let mut best_bin = 0;
    let mut best_energy = 0.0;
    for bin in 1..mono.len() / 2 {
        let energy = dft_bin_energy(&mono, bin);
        if energy > best_energy {
            best_energy = energy;
            best_bin = bin;
        }
    }
    Some(best_bin as f32 * wav.sample_rate as f32 / mono.len() as f32)
}

pub fn spectral_band_energy(wav: &WavData, lo_hz: f32, hi_hz: f32) -> f32 {
    let mono = prepare_fft_window(wav);
    if mono.len() < 8 {
        return 0.0;
    }

    let mut band = 0.0;
    let mut total = 0.0;
    for bin in 1..mono.len() / 2 {
        let freq = bin as f32 * wav.sample_rate as f32 / mono.len() as f32;
        let energy = dft_bin_energy(&mono, bin);
        total += energy;
        if freq >= lo_hz && freq <= hi_hz {
            band += energy;
        }
    }

    if total <= f32::EPSILON {
        0.0
    } else {
        band / total
    }
}

pub fn frame_time_p99(path: &Path) -> Result<f64, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("failed to read frame-time log {}: {error}", path.display()))?;
    let mut values: Vec<f64> = content.lines().filter_map(parse_log_measurement).collect();
    if values.is_empty() {
        return Err(format!("frame-time log {} had no samples", path.display()));
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let index = ((values.len() as f64 * 0.99).ceil() as usize).saturating_sub(1);
    Ok(values[index.min(values.len() - 1)])
}

pub fn xrun_count(path: &Path) -> Result<usize, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("failed to read xrun log {}: {error}", path.display()))?;
    Ok(content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count())
}

#[derive(Clone, Debug)]
pub struct StateTraceEvent {
    pub line: usize,
    pub event: Option<String>,
    pub value: Value,
}

pub fn load_state_trace(path: &Path) -> Result<Vec<StateTraceEvent>, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("failed to read state log {}: {error}", path.display()))?;
    let mut events = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed).map_err(|error| {
            format!(
                "failed to parse state log {} line {}: {error}",
                path.display(),
                line_index + 1
            )
        })?;
        let event = value
            .get("event")
            .and_then(Value::as_str)
            .map(str::to_string);
        events.push(StateTraceEvent {
            line: line_index + 1,
            event,
            value,
        });
    }
    Ok(events)
}

pub fn state_trace_contains(events: &[StateTraceEvent], expected: &[String]) -> Vec<String> {
    expected
        .iter()
        .filter(|name| {
            !events
                .iter()
                .any(|event| event.event.as_deref() == Some(name.as_str()))
        })
        .cloned()
        .collect()
}

fn sample_luminance(image: &PngImage, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, image.width.saturating_sub(1) as f32).round() as u32;
    let y = y.clamp(0.0, image.height.saturating_sub(1) as f32).round() as u32;
    image.luminance(x, y)
}

fn hdr_luminance_from_rgba(rgba: [u8; 4]) -> f32 {
    let r = srgb_to_linear(rgba[0] as f32 / 255.0);
    let g = srgb_to_linear(rgba[1] as f32 / 255.0);
    let b = srgb_to_linear(rgba[2] as f32 / 255.0);
    let ldr = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if ldr >= 0.999 {
        999.0
    } else {
        ldr / (1.0 - ldr).max(0.000001)
    }
}

fn srgb_to_linear(value: f32) -> f32 {
    value.max(0.0).powf(2.2)
}

fn prepare_fft_window(wav: &WavData) -> Vec<f32> {
    let target_len = 8192;
    let mut mono = wav.mono_samples(target_len);
    let len = mono.len().next_power_of_two() / 2;
    let len = len.clamp(8, target_len).min(mono.len());
    mono.truncate(len);
    for (i, sample) in mono.iter_mut().enumerate() {
        let window = 0.5 - 0.5 * (TAU * i as f32 / len as f32).cos();
        *sample *= window;
    }
    mono
}

fn dft_bin_energy(samples: &[f32], bin: usize) -> f32 {
    let len = samples.len() as f32;
    let mut re = 0.0;
    let mut im = 0.0;
    for (i, sample) in samples.iter().enumerate() {
        let angle = -TAU * bin as f32 * i as f32 / len;
        re += sample * angle.cos();
        im += sample * angle.sin();
    }
    re * re + im * im
}

fn parse_log_measurement(line: &str) -> Option<f64> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        for key in ["duration_ms", "frame_ms", "ms", "value"] {
            if let Some(number) = value.get(key).and_then(Value::as_f64) {
                return Some(number);
            }
        }
    }
    trimmed
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',')
        .rev()
        .find_map(|token| token.parse::<f64>().ok())
}

fn read_u16_le(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn read_u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(stem: &str, extension: &str) -> std::path::PathBuf {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "asteroids-{stem}-{}-{id}.{extension}",
            std::process::id()
        ))
    }

    #[test]
    fn decay_fit_recovers_generated_tau() {
        let tau = 0.07;
        let samples: Vec<_> = (0..80)
            .map(|i| {
                let t = i as f64 * 0.00694;
                (t, (-t / tau).exp())
            })
            .collect();

        let fit = decay_fit(&samples).expect("fit");
        assert!(
            (fit.tau_seconds - tau).abs() / tau <= 0.05,
            "tau mismatch: {fit:?}"
        );
        assert!(fit.r_squared >= 0.99, "r2 mismatch: {fit:?}");
    }

    #[test]
    fn peak_count_counts_local_maxima_over_threshold() {
        let values = [0.0, 1.0, 0.2, 0.7, 0.1, 0.9, 0.8];
        assert_eq!(peak_count(&values, 0.6), 3);
    }

    #[test]
    fn load_png_round_trips_saved_rgba() {
        let path = temp_path("png-roundtrip", "png");
        let rgba = vec![
            0, 0, 0, 255, 255, 255, 255, 255, 80, 120, 160, 255, 255, 0, 0, 255,
        ];
        save_png(&path, 2, 2, &rgba).unwrap();
        let loaded = load_png(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(loaded.width, 2);
        assert_eq!(loaded.height, 2);
        assert_eq!(loaded.rgba, rgba);
    }

    #[test]
    fn line_luminance_samples_across_requested_line() {
        let image = PngImage {
            width: 3,
            height: 1,
            rgba: vec![0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255],
        };
        let values = line_luminance(&image, (0.0, 0.0), (2.0, 0.0), 3);
        assert!(values[1] > values[0]);
        assert!(values[1] > values[2]);
    }

    #[test]
    fn load_wav_accepts_silent_pcm16_roundtrip() {
        let path = temp_path("silent", "wav");
        crate::audio::write_silent_wav(&path, 0.01).unwrap();
        let wav = load_wav(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(wav.sample_rate, crate::audio::CAPTURE_SAMPLE_RATE);
        assert_eq!(wav.channels, crate::audio::CAPTURE_CHANNELS);
        assert!(!wav.samples.is_empty());
        assert!(wav.samples.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn rms_handles_synthetic_samples() {
        let values = [1.0, -1.0, 1.0, -1.0];
        assert!((rms(&values) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn dominant_freq_finds_synthetic_sine() {
        let sample_rate = 48_000;
        let analysis_len = 4096;
        let freq = 38.0 * sample_rate as f32 / analysis_len as f32;
        let samples = sine_samples(freq, sample_rate, 8192);
        let wav = WavData {
            sample_rate,
            channels: 1,
            samples,
        };
        let actual = dominant_freq(&wav).unwrap();
        assert!(
            (actual - freq).abs() < sample_rate as f32 / analysis_len as f32,
            "actual={actual}, expected={freq}"
        );
    }

    #[test]
    fn spectral_band_energy_detects_synthetic_band() {
        let sample_rate = 48_000;
        let analysis_len = 4096;
        let freq = 85.0 * sample_rate as f32 / analysis_len as f32;
        let samples = sine_samples(freq, sample_rate, 8192);
        let wav = WavData {
            sample_rate,
            channels: 1,
            samples,
        };
        let fraction = spectral_band_energy(&wav, 900.0, 1100.0);
        assert!(fraction > 0.8, "fraction={fraction}");
    }

    #[test]
    fn frame_time_p99_parses_plain_and_json_lines() {
        let path = temp_path("frame-times", "log");
        fs::write(&path, "1.0\n{\"duration_ms\":3.0}\n2.0\n").unwrap();
        let p99 = frame_time_p99(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(p99, 3.0);
    }

    #[test]
    fn xrun_count_counts_non_empty_lines() {
        let path = temp_path("xruns", "log");
        fs::write(&path, "\nunderrun\n  \noverrun\n").unwrap();
        let count = xrun_count(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(count, 2);
    }

    #[test]
    fn load_state_trace_parses_json_lines_and_expectations() {
        let path = temp_path("state", "jsonl");
        fs::write(
            &path,
            "{\"event\":\"scenario-start\",\"tick\":0}\n{\"event\":\"tick\",\"tick\":1}\n",
        )
        .unwrap();
        let events = load_state_trace(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.as_deref(), Some("scenario-start"));
        let missing = state_trace_contains(&events, &["tick".to_string(), "ship-died".to_string()]);
        assert_eq!(missing, vec!["ship-died"]);
    }

    fn sine_samples(freq: f32, sample_rate: u32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|sample| (TAU * freq * sample as f32 / sample_rate as f32).sin())
            .collect()
    }
}
