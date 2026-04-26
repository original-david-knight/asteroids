#![allow(dead_code)]

use std::{
    fmt,
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use bytemuck::{Pod, Zeroable};
use cpal::{
    FromSample, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use fundsp::prelude::{AudioUnit, Net, Shared, U2, multizero, var};
use ringbuf::{
    Cons, HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};

pub const AUDIO_MSG_CAPACITY: usize = 1024;
pub const CAPTURE_SAMPLE_RATE: u32 = 48_000;
pub const CAPTURE_CHANNELS: u16 = 2;
pub const AUDIO_CAPTURE_BATCH_FRAMES: usize = 256;
pub const AUDIO_CAPTURE_QUEUE_BATCHES: usize = 256;
const AUDIO_CAPTURE_BATCH_SAMPLES: usize = AUDIO_CAPTURE_BATCH_FRAMES * CAPTURE_CHANNELS as usize;
const TARGET_OUTPUT_SAMPLE_RATE: u32 = 48_000;
const TARGET_OUTPUT_BLOCK_FRAMES: u32 = 256;
const AUDIO_STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const RTKIT_PRIORITY: u32 = 10;

type AudioRing = Arc<HeapRb<AudioMsg>>;
type AudioCaptureRing = Arc<HeapRb<AudioCaptureBatch>>;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct VoiceId(pub u16);

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct ParamId(pub u16);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct GameSnapshot {
    pub asteroid_count: u32,
    pub alive: u32,
    pub score: u32,
}

impl GameSnapshot {
    pub fn new(asteroid_count: u32, alive: bool, score: u32) -> Self {
        Self {
            asteroid_count,
            alive: u32::from(alive),
            score,
        }
    }

    pub fn is_alive(self) -> bool {
        self.alive != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioMsg {
    SetParam(VoiceId, ParamId, f32),
    Trigger(VoiceId),
    Release(VoiceId),
    GameState(GameSnapshot),
}

#[derive(Clone, Copy)]
pub struct AudioCaptureBatch {
    frames: u16,
    samples: [f32; AUDIO_CAPTURE_BATCH_SAMPLES],
}

impl AudioCaptureBatch {
    fn empty() -> Self {
        Self {
            frames: 0,
            samples: [0.0; AUDIO_CAPTURE_BATCH_SAMPLES],
        }
    }

    pub fn frames(self) -> usize {
        usize::from(self.frames)
    }

    pub fn samples(&self) -> &[f32] {
        let len = self.frames() * CAPTURE_CHANNELS as usize;
        &self.samples[..len]
    }
}

pub struct AudioCaptureProducer {
    producer: HeapProd<AudioCaptureBatch>,
    current: AudioCaptureBatch,
    xruns: Arc<AtomicU64>,
}

pub struct AudioCaptureConsumer {
    consumer: Cons<AudioCaptureRing>,
}

impl AudioCaptureProducer {
    pub fn push_stereo_frame_from_callback(&mut self, left: f32, right: f32) {
        let offset = usize::from(self.current.frames) * CAPTURE_CHANNELS as usize;
        self.current.samples[offset] = left;
        self.current.samples[offset + 1] = right;
        self.current.frames += 1;
        if self.current.frames() == AUDIO_CAPTURE_BATCH_FRAMES {
            self.flush_current_from_callback();
        }
    }

    pub fn push_interleaved_from_callback(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(CAPTURE_CHANNELS as usize) {
            self.push_stereo_frame_from_callback(frame[0], frame[1]);
        }
    }

    pub fn flush_current_from_callback(&mut self) {
        if self.current.frames == 0 {
            return;
        }
        let batch = self.current;
        self.current = AudioCaptureBatch::empty();
        if self.producer.try_push(batch).is_err() {
            self.xruns.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn xrun_count(&self) -> u64 {
        self.xruns.load(Ordering::Relaxed)
    }

    pub fn xrun_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.xruns)
    }
}

impl AudioCaptureConsumer {
    pub fn try_pop(&mut self) -> Option<AudioCaptureBatch> {
        self.consumer.try_pop()
    }
}

pub fn audio_capture_channel() -> (AudioCaptureProducer, AudioCaptureConsumer, Arc<AtomicU64>) {
    let ring = Arc::new(HeapRb::<AudioCaptureBatch>::new(
        AUDIO_CAPTURE_QUEUE_BATCHES,
    ));
    let (producer, _) = ring.clone().split();
    let consumer = Cons::new(ring);
    let xruns = Arc::new(AtomicU64::new(0));
    (
        AudioCaptureProducer {
            producer,
            current: AudioCaptureBatch::empty(),
            xruns: Arc::clone(&xruns),
        },
        AudioCaptureConsumer { consumer },
        xruns,
    )
}

impl AudioMsg {
    fn is_set_param(self) -> bool {
        matches!(self, Self::SetParam(_, _, _))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PushOutcome {
    Enqueued,
    DroppedOldestSetParam(AudioMsg),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PushError {
    Full(AudioMsg),
}

pub struct AudioMsgSender {
    producer: HeapProd<AudioMsg>,
    ring: AudioRing,
    dropped_set_params: AtomicU64,
}

impl AudioMsgSender {
    pub fn try_push(&mut self, msg: AudioMsg) -> Result<PushOutcome, PushError> {
        match self.producer.try_push(msg) {
            Ok(()) => Ok(PushOutcome::Enqueued),
            Err(msg) => self.push_after_dropping_oldest_set_param(msg),
        }
    }

    pub fn dropped_set_params(&self) -> u64 {
        self.dropped_set_params.load(Ordering::Relaxed)
    }

    fn push_after_dropping_oldest_set_param(
        &mut self,
        msg: AudioMsg,
    ) -> Result<PushOutcome, PushError> {
        if let Some(dropped) = self.drop_front_set_param() {
            self.dropped_set_params.fetch_add(1, Ordering::Relaxed);
            self.producer
                .try_push(msg)
                .map(|()| PushOutcome::DroppedOldestSetParam(dropped))
                .map_err(PushError::Full)
        } else if let Some(dropped) = self.replace_oldest_set_param(msg) {
            self.dropped_set_params.fetch_add(1, Ordering::Relaxed);
            Ok(PushOutcome::DroppedOldestSetParam(dropped))
        } else {
            Err(PushError::Full(msg))
        }
    }

    fn drop_front_set_param(&self) -> Option<AudioMsg> {
        let msg = self.front_msg()?;
        if !msg.is_set_param() {
            return None;
        }

        // AudioMsg is Copy and has no destructor; advancing read skips this queued parameter.
        unsafe {
            self.ring.advance_read_index(1);
        }
        Some(msg)
    }

    fn front_msg(&self) -> Option<AudioMsg> {
        let (left, right) = self.ring.occupied_slices();
        left.first()
            .or_else(|| right.first())
            .map(|slot| unsafe { *slot.assume_init_ref() })
    }

    fn replace_oldest_set_param(&self, replacement: AudioMsg) -> Option<AudioMsg> {
        let read = self.ring.read_index();
        let write = self.ring.write_index();
        let (left, right) = unsafe { self.ring.unsafe_slices_mut(read, write) };

        for slot in left.iter_mut().chain(right.iter_mut()) {
            let msg = unsafe { *slot.assume_init_ref() };
            if msg.is_set_param() {
                slot.write(replacement);
                return Some(msg);
            }
        }

        None
    }
}

pub struct AudioMsgReceiver {
    consumer: Cons<AudioRing>,
}

impl AudioMsgReceiver {
    pub fn try_pop(&mut self) -> Option<AudioMsg> {
        self.consumer.try_pop()
    }

    pub fn drain_into(&mut self, out: &mut Vec<AudioMsg>) {
        while let Some(msg) = self.try_pop() {
            out.push(msg);
        }
    }
}

pub fn audio_msg_channel() -> (AudioMsgSender, AudioMsgReceiver) {
    let ring = Arc::new(HeapRb::<AudioMsg>::new(AUDIO_MSG_CAPACITY));
    let (producer, _) = ring.clone().split();
    let consumer = Cons::new(ring.clone());
    (
        AudioMsgSender {
            producer,
            ring,
            dropped_set_params: AtomicU64::new(0),
        },
        AudioMsgReceiver { consumer },
    )
}

pub struct AtomicF32 {
    bits: AtomicU32,
}

impl AtomicF32 {
    pub fn new(value: f32) -> Self {
        Self {
            bits: AtomicU32::new(value.to_bits()),
        }
    }

    pub fn load(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    pub fn store(&self, value: f32) {
        self.bits.store(value.to_bits(), Ordering::Relaxed);
    }
}

impl Default for AtomicF32 {
    fn default() -> Self {
        Self::new(0.0)
    }
}

impl fmt::Debug for AtomicF32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AtomicF32").field(&self.load()).finish()
    }
}

pub trait Voice {
    fn id(&self) -> VoiceId;
    fn trigger(&self);
    fn release(&self);
    fn set_param(&self, param: ParamId, value: f32) -> bool;
    fn sync_controls_for_audio_block(&self);
}

pub struct PreallocatedVoice {
    id: VoiceId,
    params: Vec<AtomicF32>,
    fundsp_controls: Vec<Shared>,
    gate: AtomicBool,
}

impl PreallocatedVoice {
    pub fn new(id: VoiceId, param_defaults: &[f32]) -> Self {
        Self {
            id,
            params: param_defaults.iter().copied().map(AtomicF32::new).collect(),
            fundsp_controls: param_defaults.iter().copied().map(Shared::new).collect(),
            gate: AtomicBool::new(false),
        }
    }

    pub fn param(&self, param: ParamId) -> Option<&AtomicF32> {
        self.params.get(usize::from(param.0))
    }

    pub fn is_triggered(&self) -> bool {
        self.gate.load(Ordering::Relaxed)
    }
}

impl Voice for PreallocatedVoice {
    fn id(&self) -> VoiceId {
        self.id
    }

    fn trigger(&self) {
        self.gate.store(true, Ordering::Relaxed);
    }

    fn release(&self) {
        self.gate.store(false, Ordering::Relaxed);
    }

    fn set_param(&self, param: ParamId, value: f32) -> bool {
        let Some(cell) = self.param(param) else {
            return false;
        };
        cell.store(value);
        true
    }

    fn sync_controls_for_audio_block(&self) {
        for (cell, control) in self.params.iter().zip(self.fundsp_controls.iter()) {
            control.set(cell.load());
        }
    }
}

pub struct VoiceBank {
    voices: Vec<PreallocatedVoice>,
}

impl VoiceBank {
    pub fn single_player_default() -> Self {
        Self {
            voices: vec![
                PreallocatedVoice::new(VOICE_THRUST, &[0.0, 0.0, 0.0, 0.0]),
                PreallocatedVoice::new(VOICE_FIRE, &[0.0, 0.0, 0.0]),
                PreallocatedVoice::new(VOICE_EXPLOSION, &[0.0, 0.0, 0.0, 0.0]),
                PreallocatedVoice::new(VOICE_UFO, &[0.0, 0.0, 0.0]),
                PreallocatedVoice::new(VOICE_HEARTBEAT, &[0.0, 0.0]),
            ],
        }
    }

    pub fn len(&self) -> usize {
        self.voices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.voices.is_empty()
    }

    pub fn get(&self, id: VoiceId) -> Option<&PreallocatedVoice> {
        self.voices.iter().find(|voice| voice.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &PreallocatedVoice> {
        self.voices.iter()
    }

    pub fn control_count(&self) -> usize {
        self.voices
            .iter()
            .map(|voice| voice.fundsp_controls.len())
            .sum()
    }
}

pub const VOICE_THRUST: VoiceId = VoiceId(0);
pub const VOICE_FIRE: VoiceId = VoiceId(1);
pub const VOICE_EXPLOSION: VoiceId = VoiceId(2);
pub const VOICE_UFO: VoiceId = VoiceId(3);
pub const VOICE_HEARTBEAT: VoiceId = VoiceId(4);

pub struct AudioScaffold {
    sender: AudioMsgSender,
    receiver: AudioMsgReceiver,
    voices: VoiceBank,
}

impl AudioScaffold {
    pub fn new() -> Self {
        let (sender, receiver) = audio_msg_channel();
        Self {
            sender,
            receiver,
            voices: VoiceBank::single_player_default(),
        }
    }

    pub fn sender(&mut self) -> &mut AudioMsgSender {
        &mut self.sender
    }

    pub fn receiver(&mut self) -> &mut AudioMsgReceiver {
        &mut self.receiver
    }

    pub fn voices(&self) -> &VoiceBank {
        &self.voices
    }

    pub fn into_parts(self) -> (AudioMsgSender, AudioMsgReceiver, VoiceBank) {
        (self.sender, self.receiver, self.voices)
    }
}

impl Default for AudioScaffold {
    fn default() -> Self {
        Self::new()
    }
}

pub fn write_silent_wav(path: &Path, duration_secs: f64) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let frame_count = (duration_secs.max(0.0) * f64::from(CAPTURE_SAMPLE_RATE)).ceil() as u32;
    let data_bytes = frame_count
        .saturating_mul(u32::from(CAPTURE_CHANNELS))
        .saturating_mul(2);
    let mut writer = BufWriter::new(File::create(path)?);
    write_wav_header(&mut writer, data_bytes)?;

    let silence_frame = [0_u8; CAPTURE_CHANNELS as usize * 2];
    for _ in 0..frame_count {
        writer.write_all(&silence_frame)?;
    }
    writer.flush()
}

pub fn spawn_silent_wav_writer(path: PathBuf, duration_secs: f64) -> JoinHandle<io::Result<()>> {
    thread::spawn(move || write_silent_wav(&path, duration_secs))
}

fn write_wav_header(writer: &mut impl Write, data_bytes: u32) -> io::Result<()> {
    write_wav_header_with_format(writer, CAPTURE_SAMPLE_RATE, CAPTURE_CHANNELS, data_bytes)
}

fn write_wav_header_with_format(
    writer: &mut impl Write,
    sample_rate: u32,
    channels: u16,
    data_bytes: u32,
) -> io::Result<()> {
    let byte_rate = sample_rate * u32::from(channels) * 2;
    let block_align = channels * 2;
    writer.write_all(b"RIFF")?;
    writer.write_all(&(36_u32.saturating_add(data_bytes)).to_le_bytes())?;
    writer.write_all(b"WAVE")?;
    writer.write_all(b"fmt ")?;
    writer.write_all(&16_u32.to_le_bytes())?;
    writer.write_all(&1_u16.to_le_bytes())?;
    writer.write_all(&channels.to_le_bytes())?;
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&byte_rate.to_le_bytes())?;
    writer.write_all(&block_align.to_le_bytes())?;
    writer.write_all(&16_u16.to_le_bytes())?;
    writer.write_all(b"data")?;
    writer.write_all(&data_bytes.to_le_bytes())?;
    Ok(())
}

pub fn spawn_captured_wav_writer(
    path: PathBuf,
    duration_secs: f64,
    sample_rate: u32,
    consumer: AudioCaptureConsumer,
) -> JoinHandle<io::Result<()>> {
    thread::spawn(move || write_captured_wav(&path, duration_secs, sample_rate, consumer))
}

pub fn write_captured_wav(
    path: &Path,
    duration_secs: f64,
    sample_rate: u32,
    mut consumer: AudioCaptureConsumer,
) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let frame_count = (duration_secs.max(0.0) * f64::from(sample_rate)).ceil() as u32;
    let data_bytes = frame_count
        .saturating_mul(u32::from(CAPTURE_CHANNELS))
        .saturating_mul(2);
    let mut writer = BufWriter::new(File::create(path)?);
    write_wav_header_with_format(&mut writer, sample_rate, CAPTURE_CHANNELS, data_bytes)?;

    let mut frames_written = 0_u32;
    while frames_written < frame_count {
        let Some(batch) = consumer.try_pop() else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };
        let frames_to_write = (frame_count - frames_written).min(batch.frames() as u32) as usize;
        for frame in batch
            .samples()
            .chunks_exact(CAPTURE_CHANNELS as usize)
            .take(frames_to_write)
        {
            write_pcm16_sample(&mut writer, frame[0])?;
            write_pcm16_sample(&mut writer, frame[1])?;
        }
        frames_written += frames_to_write as u32;
    }
    writer.flush()
}

pub fn write_xrun_log(path: &Path, xrun_count: u64) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let mut writer = BufWriter::new(File::create(path)?);
    for index in 0..xrun_count {
        writeln!(writer, "xrun {index}")?;
    }
    writer.flush()
}

fn write_pcm16_sample(writer: &mut impl Write, sample: f32) -> io::Result<()> {
    let sample = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
    writer.write_all(&sample.to_le_bytes())
}

#[derive(Debug, Clone)]
pub struct AudioStreamInfo {
    pub available_hosts: Vec<String>,
    pub selected_host: String,
    pub backend_note: String,
    pub device_name: String,
    pub device_id: String,
    pub default_config: String,
    pub requested_config: String,
    pub opened_config: String,
    pub sample_format: cpal::SampleFormat,
    pub sample_rate: u32,
    pub channels: u16,
    pub requested_block_frames: u32,
    pub first_callback_frames: Option<u32>,
    pub graph_voice_count: usize,
    pub graph_control_count: usize,
    pub rtkit: RealtimePriorityOutcome,
}

impl AudioStreamInfo {
    pub fn startup_summary(&self) -> String {
        format!(
            "audio: backend={} ({}) device=\"{}\" id={} default={} requested={} opened={} sample_format={:?} first_callback_block={} graph_voices={} graph_controls={} rtkit={}",
            self.selected_host,
            self.backend_note,
            self.device_name,
            self.device_id,
            self.default_config,
            self.requested_config,
            self.opened_config,
            self.sample_format,
            self.first_callback_frames
                .map(|frames| format!("{frames} frames"))
                .unwrap_or_else(|| "not reported before startup timeout".to_string()),
            self.graph_voice_count,
            self.graph_control_count,
            self.rtkit.summary(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct RealtimePriorityOutcome {
    pub success: bool,
    pub attempted_thread_id: Option<u64>,
    pub detail: String,
}

impl RealtimePriorityOutcome {
    fn summary(&self) -> String {
        let status = if self.success {
            "SCHED_FIFO"
        } else {
            "degraded"
        };
        match self.attempted_thread_id {
            Some(thread_id) => format!("{status} tid={thread_id}: {}", self.detail),
            None => format!("{status}: {}", self.detail),
        }
    }
}

pub struct AudioRuntime {
    _stream: cpal::Stream,
    info: AudioStreamInfo,
    stream_errors: Arc<AtomicU64>,
    first_stream_error: Arc<Mutex<Option<String>>>,
}

impl AudioRuntime {
    pub fn start(
        receiver: AudioMsgReceiver,
        voices: VoiceBank,
        capture: Option<AudioCaptureProducer>,
    ) -> Result<Self, String> {
        let available_host_ids = cpal::available_hosts();
        let available_hosts = available_host_ids
            .iter()
            .map(|host| host.name().to_string())
            .collect::<Vec<_>>();
        let preferred_host = preferred_output_host(&available_host_ids)?;
        let host = cpal::host_from_id(preferred_host).map_err(|error| {
            format!(
                "failed to open cpal host {}: {error}",
                preferred_host.name()
            )
        })?;
        let selected_host = host.id().name().to_string();
        let backend_note = backend_selection_note(&selected_host, &available_hosts);

        let device = host
            .default_output_device()
            .ok_or_else(|| format!("no default cpal output device for host {selected_host}"))?;
        let device_name = device_name(&device);
        let device_id = device
            .id()
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        let default_config = device
            .default_output_config()
            .map_err(|error| format!("failed to query default output config: {error}"))?;
        let sample_format = default_config.sample_format();
        let mut stream_config = default_config.config();
        stream_config.sample_rate = TARGET_OUTPUT_SAMPLE_RATE;
        stream_config.buffer_size = cpal::BufferSize::Fixed(TARGET_OUTPUT_BLOCK_FRAMES);

        let graph_voice_count = voices.len();
        let graph_control_count = voices.control_count();
        let sample_rate = stream_config.sample_rate;
        let channels = stream_config.channels.max(1);
        let requested_config = format!("{:?}, sample_format={sample_format:?}", stream_config);
        let opened_config = requested_config.clone();

        let stream_errors = Arc::new(AtomicU64::new(0));
        let first_stream_error = Arc::new(Mutex::new(None));
        let callback_thread_id = Arc::new(AtomicU64::new(0));
        let first_callback_frames = Arc::new(AtomicU32::new(0));
        let callback_thread_id_for_callback = Arc::clone(&callback_thread_id);
        let first_callback_frames_for_callback = Arc::clone(&first_callback_frames);
        let stream_errors_for_callback = Arc::clone(&stream_errors);
        let first_stream_error_for_callback = Arc::clone(&first_stream_error);
        let mut receiver = receiver;
        let mut capture = capture;
        let mut engine = AudioEngine::new(voices, sample_rate);

        let stream = device
            .build_output_stream_raw(
                &stream_config,
                sample_format,
                move |data, _| {
                    let frames = (data.len() / usize::from(channels)) as u32;
                    if callback_thread_id_for_callback.load(Ordering::Relaxed) == 0 {
                        callback_thread_id_for_callback
                            .store(current_thread_id(), Ordering::Relaxed);
                    }
                    let _ = first_callback_frames_for_callback.compare_exchange(
                        0,
                        frames,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    );
                    render_audio_callback(
                        data,
                        usize::from(channels),
                        &mut receiver,
                        &mut engine,
                        capture.as_mut(),
                    );
                },
                move |error| {
                    stream_errors_for_callback.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut first_error) = first_stream_error_for_callback.lock()
                        && first_error.is_none()
                    {
                        *first_error = Some(error.to_string());
                    }
                },
                Some(Duration::from_millis(500)),
            )
            .map_err(|error| {
                format!(
                    "failed to build cpal output stream for requested config {requested_config}: {error}"
                )
            })?;

        stream
            .play()
            .map_err(|error| format!("failed to start cpal output stream: {error}"))?;

        let first_callback_block = wait_for_nonzero_u32(&first_callback_frames);
        let callback_tid = wait_for_nonzero_u64(&callback_thread_id);
        let rtkit = attempt_rtkit_sched_fifo(callback_tid);

        let info = AudioStreamInfo {
            available_hosts,
            selected_host,
            backend_note,
            device_name,
            device_id,
            default_config: format!("{default_config:?}"),
            requested_config,
            opened_config,
            sample_format,
            sample_rate,
            channels,
            requested_block_frames: TARGET_OUTPUT_BLOCK_FRAMES,
            first_callback_frames: first_callback_block,
            graph_voice_count,
            graph_control_count,
            rtkit,
        };

        Ok(Self {
            _stream: stream,
            info,
            stream_errors,
            first_stream_error,
        })
    }

    pub fn info(&self) -> &AudioStreamInfo {
        &self.info
    }

    pub fn sample_rate(&self) -> u32 {
        self.info.sample_rate
    }

    pub fn stream_error_count(&self) -> u64 {
        self.stream_errors.load(Ordering::Relaxed)
    }

    pub fn first_stream_error(&self) -> Option<String> {
        self.first_stream_error
            .lock()
            .ok()
            .and_then(|error| error.clone())
    }
}

struct AudioEngine {
    voices: VoiceBank,
    graph: Net,
}

impl AudioEngine {
    fn new(voices: VoiceBank, sample_rate: u32) -> Self {
        let mut graph = build_silent_voice_graph(&voices);
        graph.set_sample_rate(f64::from(sample_rate));
        graph.allocate();
        let mut warmup = [0.0_f32; 2];
        graph.tick(&[], &mut warmup);
        Self { voices, graph }
    }

    fn drain_messages(&mut self, receiver: &mut AudioMsgReceiver) {
        while let Some(msg) = receiver.try_pop() {
            match msg {
                AudioMsg::SetParam(voice_id, param_id, value) => {
                    if let Some(voice) = self.voices.get(voice_id) {
                        voice.set_param(param_id, value);
                    }
                }
                AudioMsg::Trigger(voice_id) => {
                    if let Some(voice) = self.voices.get(voice_id) {
                        voice.trigger();
                    }
                }
                AudioMsg::Release(voice_id) => {
                    if let Some(voice) = self.voices.get(voice_id) {
                        voice.release();
                    }
                }
                AudioMsg::GameState(_) => {}
            }
        }

        for voice in self.voices.iter() {
            voice.sync_controls_for_audio_block();
        }
    }

    fn next_stereo(&mut self) -> [f32; 2] {
        let mut output = [0.0_f32; 2];
        self.graph.tick(&[], &mut output);
        output
    }
}

fn build_silent_voice_graph(voices: &VoiceBank) -> Net {
    let mut graph = Net::new(0, usize::from(CAPTURE_CHANNELS));
    for voice in voices.iter() {
        for control in &voice.fundsp_controls {
            graph.push(Box::new(var(control)));
        }
    }
    let silent = graph.push(Box::new(multizero::<U2>()));
    graph.pipe_output(silent);
    graph.check();
    graph
}

fn render_audio_callback(
    data: &mut cpal::Data,
    channels: usize,
    receiver: &mut AudioMsgReceiver,
    engine: &mut AudioEngine,
    capture: Option<&mut AudioCaptureProducer>,
) {
    engine.drain_messages(receiver);
    match data.sample_format() {
        cpal::SampleFormat::I8 => render_typed::<i8>(data, channels, engine, capture),
        cpal::SampleFormat::I16 => render_typed::<i16>(data, channels, engine, capture),
        cpal::SampleFormat::I24 => render_typed::<cpal::I24>(data, channels, engine, capture),
        cpal::SampleFormat::I32 => render_typed::<i32>(data, channels, engine, capture),
        cpal::SampleFormat::I64 => render_typed::<i64>(data, channels, engine, capture),
        cpal::SampleFormat::U8 => render_typed::<u8>(data, channels, engine, capture),
        cpal::SampleFormat::U16 => render_typed::<u16>(data, channels, engine, capture),
        cpal::SampleFormat::U24 => render_typed::<cpal::U24>(data, channels, engine, capture),
        cpal::SampleFormat::U32 => render_typed::<u32>(data, channels, engine, capture),
        cpal::SampleFormat::U64 => render_typed::<u64>(data, channels, engine, capture),
        cpal::SampleFormat::F32 => render_typed::<f32>(data, channels, engine, capture),
        cpal::SampleFormat::F64 => render_typed::<f64>(data, channels, engine, capture),
        cpal::SampleFormat::DsdU8 | cpal::SampleFormat::DsdU16 | cpal::SampleFormat::DsdU32 => {
            data.bytes_mut().fill(0);
        }
        _ => data.bytes_mut().fill(0),
    }
}

fn render_typed<T>(
    data: &mut cpal::Data,
    channels: usize,
    engine: &mut AudioEngine,
    mut capture: Option<&mut AudioCaptureProducer>,
) where
    T: SizedSample + FromSample<f32> + Copy,
{
    let Some(samples) = data.as_slice_mut::<T>() else {
        data.bytes_mut().fill(0);
        return;
    };

    for frame in samples.chunks_mut(channels) {
        let [left, right] = engine.next_stereo();
        if let Some(capture) = capture.as_deref_mut() {
            capture.push_stereo_frame_from_callback(left, right);
        }
        match frame {
            [] => {}
            [mono] => {
                *mono = T::from_sample((left + right) * 0.5);
            }
            [left_out, right_out, rest @ ..] => {
                *left_out = T::from_sample(left);
                *right_out = T::from_sample(right);
                for sample in rest {
                    *sample = T::from_sample(0.0);
                }
            }
        }
    }
}

fn preferred_output_host(available_hosts: &[cpal::HostId]) -> Result<cpal::HostId, String> {
    available_hosts
        .iter()
        .copied()
        .find(|host| host.name().eq_ignore_ascii_case("PipeWire"))
        .or_else(|| {
            available_hosts
                .iter()
                .copied()
                .find(|host| host.name().eq_ignore_ascii_case("Alsa"))
        })
        .or_else(|| available_hosts.first().copied())
        .ok_or_else(|| "cpal reported no available output hosts".to_string())
}

fn backend_selection_note(selected_host: &str, available_hosts: &[String]) -> String {
    if selected_host.eq_ignore_ascii_case("PipeWire") {
        "native PipeWire host selected".to_string()
    } else if selected_host.eq_ignore_ascii_case("Alsa") {
        if available_hosts
            .iter()
            .any(|host| host.eq_ignore_ascii_case("PipeWire"))
        {
            "PipeWire host was available but ALSA was selected by fallback order".to_string()
        } else {
            "native PipeWire host unavailable in this cpal build; using ALSA direct/default, matching the probe".to_string()
        }
    } else {
        format!("PipeWire/ALSA unavailable; selected cpal host {selected_host}")
    }
}

fn device_name(device: &cpal::Device) -> String {
    if let Ok(description) = device.description() {
        let mut name = description.name().to_string();
        if let Some(driver) = description.driver() {
            name.push_str(" [driver: ");
            name.push_str(driver);
            name.push(']');
        }
        name
    } else {
        "unknown".to_string()
    }
}

fn wait_for_nonzero_u32(value: &AtomicU32) -> Option<u32> {
    let deadline = Instant::now() + AUDIO_STARTUP_TIMEOUT;
    loop {
        let observed = value.load(Ordering::Relaxed);
        if observed != 0 {
            return Some(observed);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn wait_for_nonzero_u64(value: &AtomicU64) -> Option<u64> {
    let deadline = Instant::now() + AUDIO_STARTUP_TIMEOUT;
    loop {
        let observed = value.load(Ordering::Relaxed);
        if observed != 0 {
            return Some(observed);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn attempt_rtkit_sched_fifo(thread_id: Option<u64>) -> RealtimePriorityOutcome {
    let Some(thread_id) = thread_id else {
        return RealtimePriorityOutcome {
            success: false,
            attempted_thread_id: None,
            detail: "callback thread id was not reported before startup timeout".to_string(),
        };
    };

    match rtkit_make_thread_realtime(thread_id) {
        Ok(detail) => RealtimePriorityOutcome {
            success: true,
            attempted_thread_id: Some(thread_id),
            detail,
        },
        Err(detail) => RealtimePriorityOutcome {
            success: false,
            attempted_thread_id: Some(thread_id),
            detail,
        },
    }
}

fn rtkit_make_thread_realtime(thread_id: u64) -> Result<String, String> {
    let connection = zbus::blocking::Connection::system()
        .map_err(|error| format!("connect to system DBus: {error}"))?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.RealtimeKit1",
        "/org/freedesktop/RealtimeKit1",
        "org.freedesktop.RealtimeKit1",
    )
    .map_err(|error| format!("create rtkit proxy: {error}"))?;

    proxy
        .call_method("MakeThreadRealtime", &(thread_id, RTKIT_PRIORITY))
        .map_err(|error| {
            format!(
                "call rtkit MakeThreadRealtime(thread={thread_id}, priority={RTKIT_PRIORITY}): {error}"
            )
        })?;
    Ok(format!(
        "MakeThreadRealtime(thread={thread_id}, priority={RTKIT_PRIORITY}) succeeded"
    ))
}

fn current_thread_id() -> u64 {
    unsafe { libc::syscall(libc::SYS_gettid) as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_ring_drops_oldest_set_param_and_enqueues_latest() {
        let (mut sender, mut receiver) = audio_msg_channel();

        for value in 0..AUDIO_MSG_CAPACITY {
            assert_eq!(
                sender.try_push(AudioMsg::SetParam(VOICE_THRUST, ParamId(0), value as f32)),
                Ok(PushOutcome::Enqueued)
            );
        }

        assert_eq!(
            sender.try_push(AudioMsg::SetParam(VOICE_THRUST, ParamId(0), 2048.0)),
            Ok(PushOutcome::DroppedOldestSetParam(AudioMsg::SetParam(
                VOICE_THRUST,
                ParamId(0),
                0.0
            )))
        );
        assert_eq!(sender.dropped_set_params(), 1);

        let mut drained = Vec::new();
        receiver.drain_into(&mut drained);
        assert_eq!(drained.len(), AUDIO_MSG_CAPACITY);
        assert!(matches!(
            drained.first(),
            Some(AudioMsg::SetParam(VOICE_THRUST, ParamId(0), 1.0))
        ));
        assert!(matches!(
            drained.last(),
            Some(AudioMsg::SetParam(VOICE_THRUST, ParamId(0), 2048.0))
        ));
    }

    #[test]
    fn full_ring_preserves_existing_and_incoming_critical_messages() {
        let (mut sender, mut receiver) = audio_msg_channel();

        assert_eq!(
            sender.try_push(AudioMsg::Trigger(VOICE_THRUST)),
            Ok(PushOutcome::Enqueued)
        );
        for value in 1..AUDIO_MSG_CAPACITY {
            assert_eq!(
                sender.try_push(AudioMsg::SetParam(VOICE_THRUST, ParamId(0), value as f32)),
                Ok(PushOutcome::Enqueued)
            );
        }

        assert_eq!(
            sender.try_push(AudioMsg::Release(VOICE_THRUST)),
            Ok(PushOutcome::DroppedOldestSetParam(AudioMsg::SetParam(
                VOICE_THRUST,
                ParamId(0),
                1.0
            )))
        );

        let mut drained = Vec::new();
        receiver.drain_into(&mut drained);
        assert_eq!(drained.len(), AUDIO_MSG_CAPACITY);
        assert!(matches!(
            drained.first(),
            Some(AudioMsg::Trigger(VOICE_THRUST))
        ));
        assert!(
            drained
                .iter()
                .any(|msg| matches!(msg, AudioMsg::Release(VOICE_THRUST)))
        );
        assert!(
            !drained
                .iter()
                .any(|msg| matches!(msg, AudioMsg::SetParam(VOICE_THRUST, ParamId(0), 1.0)))
        );
    }

    #[test]
    fn game_snapshot_is_pod_with_integer_alive_flag() {
        let snapshot = GameSnapshot::new(7, true, 10_000);
        assert_eq!(
            bytemuck::bytes_of(&snapshot).len(),
            size_of::<GameSnapshot>()
        );
        assert!(snapshot.is_alive());
    }
}
