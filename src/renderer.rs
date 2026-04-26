use std::{
    env,
    f32::consts::FRAC_PI_2,
    sync::{Arc, mpsc},
    time::Instant,
};

use bytemuck::{Pod, Zeroable};
use wgpu::CurrentSurfaceTexture;
use winit::{dpi::PhysicalSize, window::Window};

use crate::{
    beam::{self, BeamCommand, BeamEmitter, BeamVertex, Vec2},
    tuning,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scenario {
    #[default]
    Demo,
    Idle,
    HorizontalSweep,
    StaticBrightLine,
    StaticBrightLineLowDwell,
    StaticBrightLineHighDwell,
    GammaRamp,
}

impl Scenario {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "demo" => Some(Self::Demo),
            "idle" => Some(Self::Idle),
            "horizontal-sweep" => Some(Self::HorizontalSweep),
            "static-bright-line" => Some(Self::StaticBrightLine),
            "static-bright-line-low-dwell" => Some(Self::StaticBrightLineLowDwell),
            "static-bright-line-high-dwell" => Some(Self::StaticBrightLineHighDwell),
            "gamma-ramp" => Some(Self::GammaRamp),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Demo => "demo",
            Self::Idle => "idle",
            Self::HorizontalSweep => "horizontal-sweep",
            Self::StaticBrightLine => "static-bright-line",
            Self::StaticBrightLineLowDwell => "static-bright-line-low-dwell",
            Self::StaticBrightLineHighDwell => "static-bright-line-high-dwell",
            Self::GammaRamp => "gamma-ramp",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FrameParams {
    pub scenario: Scenario,
    pub time_seconds: f32,
    pub frame_dt_seconds: f32,
}

impl FrameParams {
    pub fn new(scenario: Scenario, time_seconds: f32, frame_dt_seconds: f32) -> Self {
        Self {
            scenario,
            time_seconds,
            frame_dt_seconds,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BloomParams {
    pub intensity: f32,
    pub threshold: f32,
}

impl BloomParams {
    pub fn new(intensity: f32, threshold: f32) -> Self {
        let intensity = if intensity.is_finite() {
            intensity.clamp(tuning::BLOOM_INTENSITY_MIN, tuning::BLOOM_INTENSITY_MAX)
        } else {
            tuning::BLOOM_INTENSITY_DEFAULT
        };
        let threshold = if threshold.is_finite() {
            threshold.clamp(tuning::BLOOM_THRESHOLD_MIN, tuning::BLOOM_THRESHOLD_MAX)
        } else {
            tuning::BLOOM_THRESHOLD_DEFAULT
        };
        Self {
            intensity,
            threshold,
        }
    }
}

impl Default for BloomParams {
    fn default() -> Self {
        Self::new(
            tuning::BLOOM_INTENSITY_DEFAULT,
            tuning::BLOOM_THRESHOLD_DEFAULT,
        )
    }
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    fullscreen_size: Option<PhysicalSize<u32>>,
    beam_pipeline: BeamLinePipeline,
    phosphor_blend_pipeline: PhosphorBlendPipeline,
    bloom_pipeline: BloomPipeline,
    composite_pipeline: CompositePipeline,
    phosphor: PhosphorTargets,
    phosphor_bind_groups: PhosphorBindGroups,
    bloom: BloomTargets,
    bloom_bind_groups: BloomBindGroups,
    beam_emitter: BeamEmitter,
    gameplay_beam_emitter: BeamEmitter,
    phosphor_tau_ms: f32,
    bloom_params: BloomParams,
    last_frame: Instant,
    demo_start: Instant,
}

pub struct HeadlessRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: PhysicalSize<u32>,
    output: OutputTexture,
    beam_pipeline: BeamLinePipeline,
    phosphor_blend_pipeline: PhosphorBlendPipeline,
    bloom_pipeline: BloomPipeline,
    composite_pipeline: CompositePipeline,
    phosphor: PhosphorTargets,
    phosphor_bind_groups: PhosphorBindGroups,
    bloom: BloomTargets,
    bloom_bind_groups: BloomBindGroups,
    beam_emitter: BeamEmitter,
    gameplay_beam_emitter: BeamEmitter,
    phosphor_tau_ms: f32,
    bloom_params: BloomParams,
}

impl Renderer {
    pub async fn new(
        window: Arc<Window>,
        fullscreen_size: Option<PhysicalSize<u32>>,
    ) -> Result<Self, String> {
        let size = target_surface_size(&window, fullscreen_size);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(|error| format!("failed to create wgpu surface: {error}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| format!("failed to find a surface-compatible GPU adapter: {error}"))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Asteroids Device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .map_err(|error| format!("failed to create wgpu device: {error}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = choose_surface_format(&caps.formats)
            .ok_or_else(|| "surface reported no presentable texture formats".to_string())?;
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Fifo) {
            wgpu::PresentMode::Fifo
        } else {
            wgpu::PresentMode::AutoVsync
        };
        let alpha_mode = caps
            .alpha_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::CompositeAlphaMode::Opaque)
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode,
            desired_maximum_frame_latency: 1,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let phosphor_config = choose_phosphor_format(&adapter);
        if phosphor_config.fallback {
            eprintln!(
                "phosphor accumulator: Rgba16Float unavailable; using {:?} with max_luma={:.1}",
                phosphor_config.format, phosphor_config.max_luma
            );
        }

        let beam_pipeline = BeamLinePipeline::new(&device, phosphor_config.format);
        let phosphor_blend_pipeline = PhosphorBlendPipeline::new(&device, phosphor_config.format);
        let bloom_pipeline = BloomPipeline::new(&device, phosphor_config.format);
        let composite_pipeline = CompositePipeline::new(
            &device,
            format,
            phosphor_config.format,
            phosphor_config.format,
        );
        let phosphor = PhosphorTargets::new(&device, size, phosphor_config);
        let bloom = BloomTargets::new(&device, size, phosphor_config.format);
        let phosphor_bind_groups = PhosphorBindGroups::new(
            &device,
            &phosphor,
            &bloom,
            &phosphor_blend_pipeline,
            &composite_pipeline,
        );
        let bloom_bind_groups = BloomBindGroups::new(&device, &phosphor, &bloom, &bloom_pipeline);

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            fullscreen_size,
            beam_pipeline,
            phosphor_blend_pipeline,
            bloom_pipeline,
            composite_pipeline,
            phosphor,
            phosphor_bind_groups,
            bloom,
            bloom_bind_groups,
            beam_emitter: BeamEmitter::new(),
            gameplay_beam_emitter: BeamEmitter::new(),
            phosphor_tau_ms: tuning::PHOSPHOR_TAU_DEFAULT_MS,
            bloom_params: BloomParams::default(),
            last_frame: Instant::now(),
            demo_start: Instant::now(),
        })
    }

    pub fn resize_for_window(&mut self, window: &Window) {
        let size = target_surface_size(window, self.fullscreen_size);
        if size.width == 0 || size.height == 0 || size == self.size {
            return;
        }

        self.size = size;
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.phosphor = PhosphorTargets::new(&self.device, size, self.phosphor.config);
        self.bloom = BloomTargets::new(&self.device, size, self.phosphor.config.format);
        self.phosphor_bind_groups = PhosphorBindGroups::new(
            &self.device,
            &self.phosphor,
            &self.bloom,
            &self.phosphor_blend_pipeline,
            &self.composite_pipeline,
        );
        self.bloom_bind_groups = BloomBindGroups::new(
            &self.device,
            &self.phosphor,
            &self.bloom,
            &self.bloom_pipeline,
        );
        self.last_frame = Instant::now();
    }

    pub fn render(&mut self) -> Result<(), String> {
        let frame_dt_seconds = self.frame_dt_seconds();
        let params = FrameParams::new(
            Scenario::Demo,
            self.demo_start.elapsed().as_secs_f32(),
            frame_dt_seconds,
        );
        self.render_with_params(params)
    }

    pub fn render_with_params(&mut self, params: FrameParams) -> Result<(), String> {
        emit_frame_beams(
            &mut self.beam_emitter,
            &mut self.gameplay_beam_emitter,
            params.scenario,
            params.time_seconds,
            self.size,
        );

        let frame = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) | CurrentSurfaceTexture::Suboptimal(frame) => {
                frame
            }
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => return Ok(()),
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            CurrentSurfaceTexture::Validation => {
                return Err("surface validation failed while acquiring the next frame".to_string());
            }
        };

        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Asteroids Phosphor Encoder"),
            });

        encode_scene_to_view(SceneRenderContext {
            device: &self.device,
            queue: &self.queue,
            size: self.size,
            beam_pipeline: &mut self.beam_pipeline,
            phosphor_blend_pipeline: &mut self.phosphor_blend_pipeline,
            bloom_pipeline: &self.bloom_pipeline,
            composite_pipeline: &self.composite_pipeline,
            phosphor: &mut self.phosphor,
            phosphor_bind_groups: &self.phosphor_bind_groups,
            bloom: &self.bloom,
            bloom_bind_groups: &self.bloom_bind_groups,
            beam_emitter: &self.beam_emitter,
            phosphor_tau_ms: self.phosphor_tau_ms,
            bloom_params: self.bloom_params,
            frame_dt_seconds: params.frame_dt_seconds,
            target_view: &surface_view,
            encoder: &mut encoder,
        });

        self.queue.submit([encoder.finish()]);
        frame.present();
        self.phosphor.advance();
        Ok(())
    }

    pub fn adjust_phosphor_tau_ms(&mut self, delta_ms: f32) -> f32 {
        self.phosphor_tau_ms = (self.phosphor_tau_ms + delta_ms)
            .clamp(tuning::PHOSPHOR_TAU_MIN_MS, tuning::PHOSPHOR_TAU_MAX_MS);
        self.phosphor_tau_ms
    }

    pub fn reset_phosphor_tau_ms(&mut self) -> f32 {
        self.phosphor_tau_ms = tuning::PHOSPHOR_TAU_DEFAULT_MS;
        self.phosphor_tau_ms
    }

    pub fn set_bloom_params(&mut self, intensity: f32, threshold: f32) {
        self.bloom_params = BloomParams::new(intensity, threshold);
    }

    pub fn adjust_bloom_intensity(&mut self, delta: f32) -> f32 {
        self.bloom_params.intensity = (self.bloom_params.intensity + delta)
            .clamp(tuning::BLOOM_INTENSITY_MIN, tuning::BLOOM_INTENSITY_MAX);
        self.bloom_params.intensity
    }

    pub fn adjust_bloom_threshold(&mut self, delta: f32) -> f32 {
        self.bloom_params.threshold = (self.bloom_params.threshold + delta)
            .clamp(tuning::BLOOM_THRESHOLD_MIN, tuning::BLOOM_THRESHOLD_MAX);
        self.bloom_params.threshold
    }

    pub fn reset_bloom_params(&mut self) -> BloomParams {
        self.bloom_params = BloomParams::default();
        self.bloom_params
    }

    pub fn size(&self) -> PhysicalSize<u32> {
        self.size
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn phosphor_format(&self) -> wgpu::TextureFormat {
        self.phosphor.format()
    }

    pub fn phosphor_tau_ms(&self) -> f32 {
        self.phosphor_tau_ms
    }

    pub fn bloom_params(&self) -> BloomParams {
        self.bloom_params
    }

    pub fn present_mode(&self) -> wgpu::PresentMode {
        self.config.present_mode
    }

    fn frame_dt_seconds(&mut self) -> f32 {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        if dt.is_finite() {
            dt.clamp(1.0 / 1000.0, 0.1)
        } else {
            1.0 / 144.0
        }
    }
}

impl HeadlessRenderer {
    pub async fn new(size: PhysicalSize<u32>) -> Result<Self, String> {
        let size = PhysicalSize::new(size.width.max(1), size.height.max(1));
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| format!("failed to find a headless GPU adapter: {error}"))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Asteroids Headless Device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .map_err(|error| format!("failed to create headless wgpu device: {error}"))?;

        let phosphor_config = choose_phosphor_format(&adapter);
        if phosphor_config.fallback {
            eprintln!(
                "headless phosphor accumulator: Rgba16Float unavailable; using {:?} with max_luma={:.1}",
                phosphor_config.format, phosphor_config.max_luma
            );
        }

        let output_format = wgpu::TextureFormat::Rgba8Unorm;
        let beam_pipeline = BeamLinePipeline::new(&device, phosphor_config.format);
        let phosphor_blend_pipeline = PhosphorBlendPipeline::new(&device, phosphor_config.format);
        let bloom_pipeline = BloomPipeline::new(&device, phosphor_config.format);
        let composite_pipeline = CompositePipeline::new(
            &device,
            output_format,
            phosphor_config.format,
            phosphor_config.format,
        );
        let phosphor = PhosphorTargets::new(&device, size, phosphor_config);
        let bloom = BloomTargets::new(&device, size, phosphor_config.format);
        let phosphor_bind_groups = PhosphorBindGroups::new(
            &device,
            &phosphor,
            &bloom,
            &phosphor_blend_pipeline,
            &composite_pipeline,
        );
        let bloom_bind_groups = BloomBindGroups::new(&device, &phosphor, &bloom, &bloom_pipeline);
        let output = OutputTexture::new(&device, size, output_format);

        Ok(Self {
            device,
            queue,
            size,
            output,
            beam_pipeline,
            phosphor_blend_pipeline,
            bloom_pipeline,
            composite_pipeline,
            phosphor,
            phosphor_bind_groups,
            bloom,
            bloom_bind_groups,
            beam_emitter: BeamEmitter::new(),
            gameplay_beam_emitter: BeamEmitter::new(),
            phosphor_tau_ms: tuning::PHOSPHOR_TAU_DEFAULT_MS,
            bloom_params: BloomParams::default(),
        })
    }

    pub fn render(&mut self, params: FrameParams) -> Result<(), String> {
        emit_frame_beams(
            &mut self.beam_emitter,
            &mut self.gameplay_beam_emitter,
            params.scenario,
            params.time_seconds,
            self.size,
        );

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Asteroids Headless Phosphor Encoder"),
            });

        encode_scene_to_view(SceneRenderContext {
            device: &self.device,
            queue: &self.queue,
            size: self.size,
            beam_pipeline: &mut self.beam_pipeline,
            phosphor_blend_pipeline: &mut self.phosphor_blend_pipeline,
            bloom_pipeline: &self.bloom_pipeline,
            composite_pipeline: &self.composite_pipeline,
            phosphor: &mut self.phosphor,
            phosphor_bind_groups: &self.phosphor_bind_groups,
            bloom: &self.bloom,
            bloom_bind_groups: &self.bloom_bind_groups,
            beam_emitter: &self.beam_emitter,
            phosphor_tau_ms: self.phosphor_tau_ms,
            bloom_params: self.bloom_params,
            frame_dt_seconds: params.frame_dt_seconds,
            target_view: &self.output.view,
            encoder: &mut encoder,
        });

        self.queue.submit([encoder.finish()]);
        self.phosphor.advance();
        Ok(())
    }

    pub fn set_bloom_params(&mut self, intensity: f32, threshold: f32) {
        self.bloom_params = BloomParams::new(intensity, threshold);
    }

    pub fn capture_rgba8(&self) -> Result<Vec<u8>, String> {
        let bytes_per_pixel = 4;
        let unpadded_bytes_per_row = self.size.width * bytes_per_pixel;
        let padded_bytes_per_row = align_to_copy_bytes_per_row(unpadded_bytes_per_row);
        let buffer_size = u64::from(padded_bytes_per_row) * u64::from(self.size.height);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Asteroids Headless Readback Buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Asteroids Headless Readback Encoder"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.output.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(self.size.height),
                },
            },
            wgpu::Extent3d {
                width: self.size.width,
                height: self.size.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = readback.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result.map_err(|error| error.to_string()));
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| format!("failed while polling readback buffer: {error}"))?;
        receiver
            .recv()
            .map_err(|error| format!("failed to receive readback map result: {error}"))?
            .map_err(|error| format!("failed to map readback buffer: {error}"))?;

        let mapped = slice.get_mapped_range();
        let mut rgba = vec![0; (self.size.width * self.size.height * bytes_per_pixel) as usize];
        for row in 0..self.size.height as usize {
            let src_start = row * padded_bytes_per_row as usize;
            let src_end = src_start + unpadded_bytes_per_row as usize;
            let dst_start = row * unpadded_bytes_per_row as usize;
            let dst_end = dst_start + unpadded_bytes_per_row as usize;
            rgba[dst_start..dst_end].copy_from_slice(&mapped[src_start..src_end]);
        }
        drop(mapped);
        readback.unmap();
        Ok(rgba)
    }

    pub fn size(&self) -> PhysicalSize<u32> {
        self.size
    }

    pub fn phosphor_format(&self) -> wgpu::TextureFormat {
        self.phosphor.format()
    }
}

struct SceneRenderContext<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    size: PhysicalSize<u32>,
    beam_pipeline: &'a mut BeamLinePipeline,
    phosphor_blend_pipeline: &'a mut PhosphorBlendPipeline,
    bloom_pipeline: &'a BloomPipeline,
    composite_pipeline: &'a CompositePipeline,
    phosphor: &'a mut PhosphorTargets,
    phosphor_bind_groups: &'a PhosphorBindGroups,
    bloom: &'a BloomTargets,
    bloom_bind_groups: &'a BloomBindGroups,
    beam_emitter: &'a BeamEmitter,
    phosphor_tau_ms: f32,
    bloom_params: BloomParams,
    frame_dt_seconds: f32,
    target_view: &'a wgpu::TextureView,
    encoder: &'a mut wgpu::CommandEncoder,
}

fn encode_scene_to_view(ctx: SceneRenderContext<'_>) {
    ctx.beam_pipeline.upload(
        ctx.device,
        ctx.queue,
        ctx.beam_emitter.commands(),
        beam_quad_half_width_ndc(ctx.size),
        ctx.size,
        ctx.phosphor.max_luma(),
    );
    ctx.phosphor_blend_pipeline.update_uniforms(
        ctx.queue,
        ctx.frame_dt_seconds,
        ctx.phosphor_tau_ms * 0.001,
        ctx.phosphor.max_luma(),
    );
    ctx.bloom_bind_groups
        .update_downsample_thresholds(ctx.queue, ctx.bloom_params.threshold);
    ctx.composite_pipeline
        .update_uniforms(ctx.queue, ctx.bloom_params.intensity);

    if ctx.phosphor.needs_clear() {
        ctx.phosphor.encode_clear(ctx.encoder);
        ctx.phosphor.mark_clear();
    }

    let target_index = ctx.phosphor.target_index();

    {
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Asteroids Beam SDF Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.phosphor.view(target_index),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        ctx.beam_pipeline.draw(&mut pass);
    }

    ctx.phosphor
        .copy_target_to_beam_scratch(ctx.encoder, target_index);

    {
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Asteroids Phosphor Decay Blend Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.phosphor.view(target_index),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        ctx.phosphor_blend_pipeline
            .draw(&mut pass, ctx.phosphor_bind_groups.blend(target_index));
    }

    ctx.bloom_pipeline
        .encode(ctx.encoder, ctx.bloom, ctx.bloom_bind_groups, target_index);

    {
        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Asteroids Composite Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        ctx.composite_pipeline
            .draw(&mut pass, ctx.phosphor_bind_groups.composite(target_index));
    }
}

fn align_to_copy_bytes_per_row(bytes_per_row: u32) -> u32 {
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    bytes_per_row.div_ceil(alignment) * alignment
}

// Aspect strategy is locked to DESIGN.md Open Question 1 option (c):
// centered 4:3 gameplay with score/lives vector readout bezels in the side margins.
// This follows DESIGN.md's default lean and keeps the autonomous run from making
// a subjective taste call between the recorded alternatives.
const PLAYFIELD_ASPECT_RATIO: f32 = 4.0 / 3.0;
const BEZEL_READOUT_INTENSITY: f32 = 0.56;
const BEZEL_READOUT_DWELL_US: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct PlayfieldRect {
    min: Vec2,
    max: Vec2,
}

impl PlayfieldRect {
    fn centered_4_3(size: PhysicalSize<u32>) -> Self {
        let width = size.width.max(1) as f32;
        let height = size.height.max(1) as f32;
        let target_aspect = width / height;

        if target_aspect >= PLAYFIELD_ASPECT_RATIO {
            let half_width = PLAYFIELD_ASPECT_RATIO / target_aspect;
            Self {
                min: Vec2::new(-half_width, -1.0),
                max: Vec2::new(half_width, 1.0),
            }
        } else {
            let half_height = target_aspect / PLAYFIELD_ASPECT_RATIO;
            Self {
                min: Vec2::new(-1.0, -half_height),
                max: Vec2::new(1.0, half_height),
            }
        }
    }

    fn map_point(self, point: Vec2) -> Vec2 {
        let center = (self.min + self.max) * 0.5;
        let half_extent = (self.max - self.min) * 0.5;
        center + Vec2::new(point.x * half_extent.x, point.y * half_extent.y)
    }

    fn map_command(self, command: BeamCommand) -> BeamCommand {
        BeamCommand {
            start: self.map_point(command.start),
            end: self.map_point(command.end),
            intensity: command.intensity,
            dwell_us: command.dwell_us,
        }
    }

    fn left_margin(self) -> Option<NdcRect> {
        (self.min.x > -1.0).then_some(NdcRect {
            min: Vec2::new(-1.0, -1.0),
            max: Vec2::new(self.min.x, 1.0),
        })
    }

    fn right_margin(self) -> Option<NdcRect> {
        (self.max.x < 1.0).then_some(NdcRect {
            min: Vec2::new(self.max.x, -1.0),
            max: Vec2::new(1.0, 1.0),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct NdcRect {
    min: Vec2,
    max: Vec2,
}

impl NdcRect {
    fn width(self) -> f32 {
        self.max.x - self.min.x
    }

    fn center_x(self) -> f32 {
        (self.min.x + self.max.x) * 0.5
    }
}

fn emit_frame_beams(
    frame_emitter: &mut BeamEmitter,
    gameplay_emitter: &mut BeamEmitter,
    scenario: Scenario,
    time_s: f32,
    size: PhysicalSize<u32>,
) {
    let playfield = PlayfieldRect::centered_4_3(size);

    frame_emitter.clear();
    gameplay_emitter.clear();

    emit_bezel_readouts(frame_emitter, playfield, size);
    emit_scenario_beams(gameplay_emitter, scenario, time_s);

    for command in gameplay_emitter.commands() {
        frame_emitter.emit(playfield.map_command(*command));
    }
}

fn emit_bezel_readouts(
    emitter: &mut BeamEmitter,
    playfield: PlayfieldRect,
    size: PhysicalSize<u32>,
) {
    if let Some(left) = playfield.left_margin() {
        emit_score_readout(emitter, left, size);
    }
    if let Some(right) = playfield.right_margin() {
        emit_lives_readout(emitter, right);
    }
}

fn emit_score_readout(emitter: &mut BeamEmitter, margin: NdcRect, size: PhysicalSize<u32>) {
    let aspect_correction = size.height.max(1) as f32 / size.width.max(1) as f32;
    let natural_digit_height = 0.082;
    let natural_digit_width = natural_digit_height * 0.56 * aspect_correction;
    let digit_gap_ratio = 0.36;
    let digits = 6.0;
    let gaps = digits - 1.0;
    let max_digit_width = margin.width() * 0.78 / (digits + gaps * digit_gap_ratio);
    let digit_width = natural_digit_width.min(max_digit_width).max(0.006);
    let digit_height = digit_width / (0.56 * aspect_correction).max(0.001);
    let gap = digit_width * digit_gap_ratio;
    let total_width = digit_width * digits + gap * gaps;
    let start_x = margin.center_x() - total_width * 0.5;
    let bottom_y = 0.72;

    for index in 0..6 {
        emit_seven_segment_digit(
            emitter,
            0,
            Vec2::new(start_x + index as f32 * (digit_width + gap), bottom_y),
            Vec2::new(digit_width, digit_height),
            BEZEL_READOUT_INTENSITY,
        );
    }
}

fn emit_lives_readout(emitter: &mut BeamEmitter, margin: NdcRect) {
    let icon_scale = (margin.width() * 0.62 / 0.44).clamp(0.055, 0.16);
    let x = margin.center_x();
    for y in [0.80, 0.66, 0.52] {
        emit_ship_outline(
            emitter,
            Vec2::new(x, y),
            FRAC_PI_2,
            icon_scale,
            BEZEL_READOUT_INTENSITY,
        );
    }
}

fn emit_seven_segment_digit(
    emitter: &mut BeamEmitter,
    digit: u8,
    origin: Vec2,
    size: Vec2,
    intensity: f32,
) {
    let segments = match digit {
        0 => [true, true, true, false, true, true, true],
        1 => [false, false, true, false, false, true, false],
        2 => [true, false, true, true, true, false, true],
        3 => [true, false, true, true, false, true, true],
        4 => [false, true, true, true, false, true, false],
        5 => [true, true, false, true, false, true, true],
        6 => [true, true, false, true, true, true, true],
        7 => [true, false, true, false, false, true, false],
        8 => [true, true, true, true, true, true, true],
        9 => [true, true, true, true, false, true, true],
        _ => [false; 7],
    };

    let x0 = origin.x;
    let x1 = origin.x + size.x;
    let y0 = origin.y;
    let y1 = origin.y + size.y;
    let ym = (y0 + y1) * 0.5;
    let inset = size.x * 0.16;
    let vertical_gap = size.y * 0.08;

    let segment_points = [
        (Vec2::new(x0 + inset, y1), Vec2::new(x1 - inset, y1)),
        (
            Vec2::new(x0, ym + vertical_gap),
            Vec2::new(x0, y1 - vertical_gap),
        ),
        (
            Vec2::new(x1, ym + vertical_gap),
            Vec2::new(x1, y1 - vertical_gap),
        ),
        (Vec2::new(x0 + inset, ym), Vec2::new(x1 - inset, ym)),
        (
            Vec2::new(x0, y0 + vertical_gap),
            Vec2::new(x0, ym - vertical_gap),
        ),
        (
            Vec2::new(x1, y0 + vertical_gap),
            Vec2::new(x1, ym - vertical_gap),
        ),
        (Vec2::new(x0 + inset, y0), Vec2::new(x1 - inset, y0)),
    ];

    for (enabled, (start, end)) in segments.into_iter().zip(segment_points) {
        if enabled {
            emitter.emit_segment(start, end, intensity, BEZEL_READOUT_DWELL_US);
        }
    }
}

fn emit_scenario_beams(emitter: &mut BeamEmitter, scenario: Scenario, time_s: f32) {
    match scenario {
        Scenario::Demo => emit_demo_beams(emitter, time_s),
        Scenario::Idle => emit_idle_beams(emitter),
        Scenario::HorizontalSweep => emit_horizontal_sweep_beams(emitter, time_s),
        Scenario::StaticBrightLine => {
            emit_static_bright_line(emitter, tuning::SHIP_OUTLINE_SEGMENT_DWELL_US)
        }
        Scenario::StaticBrightLineLowDwell => {
            emit_static_bright_line(emitter, tuning::PHOSPHOR_TRAIL_LOW_DWELL_US)
        }
        Scenario::StaticBrightLineHighDwell => {
            emit_static_bright_line(emitter, tuning::PHOSPHOR_TRAIL_HIGH_DWELL_US)
        }
        Scenario::GammaRamp => emit_gamma_ramp_beams(emitter),
    }
}

fn emit_idle_beams(emitter: &mut BeamEmitter) {
    emit_ship_outline(emitter, Vec2::ZERO, 0.0, 0.55, 0.85);
    emitter.emit_bullet_dot(Vec2::new(0.56, -0.34), 0.014, 0.7);
}

fn emit_horizontal_sweep_beams(emitter: &mut BeamEmitter, time_s: f32) {
    let period_s = 1.8;
    let phase = (time_s / period_s).rem_euclid(1.0);
    let x = -0.92 + phase * 1.84;
    let half_len = 0.14;
    emitter.emit_segment_with_endpoint_bonus(
        Vec2::new(x - half_len, 0.0),
        Vec2::new(x + half_len, 0.0),
        1.0,
        tuning::SHIP_OUTLINE_SEGMENT_DWELL_US,
    );
}

fn emit_static_bright_line(emitter: &mut BeamEmitter, dwell_us: f32) {
    emitter.emit_segment(Vec2::new(-0.55, 0.0), Vec2::new(0.55, 0.0), 1.0, dwell_us);
}

fn emit_gamma_ramp_beams(emitter: &mut BeamEmitter) {
    let bars = 17;
    for i in 0..bars {
        let t = i as f32 / (bars - 1) as f32;
        let x = -0.78 + t * 1.56;
        emitter.emit_segment(
            Vec2::new(x, -0.38),
            Vec2::new(x, 0.38),
            t,
            tuning::SHIP_OUTLINE_SEGMENT_DWELL_US,
        );
    }
}

fn emit_ship_outline(
    emitter: &mut BeamEmitter,
    center: Vec2,
    angle: f32,
    scale: f32,
    intensity: f32,
) {
    let (angle_sin, angle_cos) = angle.sin_cos();
    let nose_direction = Vec2::new(angle_cos, angle_sin);
    let side_direction = nose_direction.left_perp();

    let nose = center + nose_direction * (0.44 * scale);
    let left = center - nose_direction * (0.30 * scale) + side_direction * (0.22 * scale);
    let right = center - nose_direction * (0.30 * scale) - side_direction * (0.22 * scale);
    let notch = center - nose_direction * (0.12 * scale);

    emitter
        .emit(
            BeamCommand::builder(nose, left)
                .intensity(intensity)
                .dwell_us(tuning::SHIP_OUTLINE_SEGMENT_DWELL_US)
                .endpoint_dwell_bonus()
                .build(),
        )
        .emit_ship_outline_segment(left, notch, intensity)
        .emit_segment_with_endpoint_bonus(
            notch,
            right,
            intensity,
            tuning::SHIP_OUTLINE_SEGMENT_DWELL_US,
        )
        .emit_segment_with_endpoint_bonus(
            right,
            nose,
            intensity,
            tuning::SHIP_OUTLINE_SEGMENT_DWELL_US,
        );
}

fn emit_demo_beams(emitter: &mut BeamEmitter, time_s: f32) {
    let center = Vec2::ZERO + Vec2::new((time_s * 0.47).sin() * 0.24, (time_s * 0.31).cos() * 0.16);
    let angle = time_s * 1.65;
    let (angle_sin, angle_cos) = angle.sin_cos();
    let nose_direction = Vec2::new(angle_cos, angle_sin);
    let side_direction = nose_direction.left_perp();

    let nose = center + nose_direction * 0.44;
    let left = center - nose_direction * 0.30 + side_direction * 0.22;
    let right = center - nose_direction * 0.30 - side_direction * 0.22;
    let notch = center - nose_direction * 0.12;

    emitter
        .emit(
            BeamCommand::builder(nose, left)
                .intensity(1.0)
                .dwell_us(tuning::SHIP_OUTLINE_SEGMENT_DWELL_US)
                .endpoint_dwell_bonus()
                .build(),
        )
        .emit_ship_outline_segment(left, notch, 1.0)
        .emit_segment_with_endpoint_bonus(notch, right, 1.0, tuning::SHIP_OUTLINE_SEGMENT_DWELL_US)
        .emit_segment_with_endpoint_bonus(right, nose, 1.0, tuning::SHIP_OUTLINE_SEGMENT_DWELL_US);

    let sweep_x = (time_s * 0.73).sin() * 0.78;
    emitter
        .emit_bullet_dot(Vec2::new(sweep_x, -0.54), 0.018, 1.0)
        .emit_asteroid_hull_segment(Vec2::new(-0.86, 0.62), Vec2::new(-0.58, 0.70), 0.45);

    emit_phosphor_trail_verification_beams(emitter, time_s);
}

fn emit_phosphor_trail_verification_beams(emitter: &mut BeamEmitter, time_s: f32) {
    let drift = (time_s * 0.55).sin() * 0.12;
    let x0 = -0.52 + drift;
    let length = 0.42;
    let slope = (time_s * 1.2).sin() * 0.035;
    let rows = [
        (-0.78, tuning::PHOSPHOR_TRAIL_LOW_DWELL_US),
        (-0.86, tuning::PHOSPHOR_TRAIL_MID_DWELL_US),
        (-0.94, tuning::PHOSPHOR_TRAIL_HIGH_DWELL_US),
    ];

    for (y, dwell_us) in rows {
        emitter.emit_segment(
            Vec2::new(x0, y),
            Vec2::new(x0 + length, y + slope),
            1.0,
            dwell_us,
        );
    }
}

pub fn target_surface_size(
    window: &Window,
    fullscreen_size: Option<PhysicalSize<u32>>,
) -> PhysicalSize<u32> {
    if let Some(size) = fullscreen_size.filter(|size| size.width > 0 && size.height > 0) {
        return size;
    }

    let monitor_size = window
        .current_monitor()
        .or_else(|| window.primary_monitor())
        .map(|monitor| monitor.size())
        .filter(|size| size.width > 0 && size.height > 0);
    monitor_size.unwrap_or_else(|| {
        let inner = window.inner_size();
        PhysicalSize::new(inner.width.max(1), inner.height.max(1))
    })
}

pub fn display_server_note() -> String {
    let session = env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_string());
    let desktop = env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".to_string());

    if session.eq_ignore_ascii_case("wayland") && desktop.to_lowercase().contains("hyprland") {
        "Wayland/Hyprland borderless fullscreen (probe PASS; no X11 fallback forced)".to_string()
    } else if session.eq_ignore_ascii_case("x11") {
        "X11 borderless fullscreen".to_string()
    } else {
        format!("{session}/{desktop} borderless fullscreen")
    }
}

fn choose_surface_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    [
        wgpu::TextureFormat::Rgba16Float,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ]
    .into_iter()
    .find(|format| formats.contains(format))
}

fn choose_phosphor_format(adapter: &wgpu::Adapter) -> PhosphorFormatConfig {
    let required_usages = wgpu::TextureUsages::RENDER_ATTACHMENT
        | wgpu::TextureUsages::TEXTURE_BINDING
        | wgpu::TextureUsages::COPY_SRC
        | wgpu::TextureUsages::COPY_DST;
    let rgba16_features = adapter.get_texture_format_features(wgpu::TextureFormat::Rgba16Float);
    if rgba16_features.allowed_usages.contains(required_usages) {
        PhosphorFormatConfig {
            format: wgpu::TextureFormat::Rgba16Float,
            max_luma: tuning::PHOSPHOR_MAX_LUMA,
            fallback: false,
        }
    } else {
        PhosphorFormatConfig {
            format: wgpu::TextureFormat::Rgba8Unorm,
            max_luma: tuning::PHOSPHOR_FALLBACK_MAX_LUMA,
            fallback: true,
        }
    }
}

fn beam_quad_half_width_ndc(size: PhysicalSize<u32>) -> f32 {
    let min_axis = size.width.min(size.height).max(1) as f32;
    tuning::BEAM_QUAD_HALF_WIDTH_PIXELS * 2.0 / min_axis
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct BeamUniforms {
    target_size_sigma_dwell: [f32; 4],
    growth_max_luma_pad: [f32; 4],
}

impl BeamUniforms {
    fn new(size: PhysicalSize<u32>, max_luma: f32) -> Self {
        Self {
            target_size_sigma_dwell: [
                size.width.max(1) as f32,
                size.height.max(1) as f32,
                tuning::BEAM_SIGMA_PIXELS,
                tuning::SHIP_OUTLINE_SEGMENT_DWELL_US,
            ],
            growth_max_luma_pad: [tuning::BEAM_SIGMA_DWELL_GROWTH, max_luma, 0.0, 0.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct PhosphorBlendUniforms {
    frame_dt_tau_max_luma: [f32; 4],
}

impl PhosphorBlendUniforms {
    fn new(frame_dt_seconds: f32, tau_seconds: f32, max_luma: f32) -> Self {
        Self {
            frame_dt_tau_max_luma: [
                frame_dt_seconds.max(0.0),
                tau_seconds.max(0.0001),
                max_luma,
                0.0,
            ],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct BloomDownsampleUniforms {
    threshold_pad: [f32; 4],
}

impl BloomDownsampleUniforms {
    fn new(threshold: f32) -> Self {
        Self {
            threshold_pad: [threshold.max(0.0), 0.0, 0.0, 0.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CompositeUniforms {
    bloom_intensity_pad: [f32; 4],
}

impl CompositeUniforms {
    fn new(bloom_intensity: f32) -> Self {
        Self {
            bloom_intensity_pad: [bloom_intensity.max(0.0), 0.0, 0.0, 0.0],
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PhosphorFormatConfig {
    format: wgpu::TextureFormat,
    max_luma: f32,
    fallback: bool,
}

struct PhosphorTargets {
    config: PhosphorFormatConfig,
    size: PhysicalSize<u32>,
    history: [PhosphorTexture; 2],
    beam_scratch: PhosphorTexture,
    previous_index: usize,
    needs_clear: bool,
}

impl PhosphorTargets {
    fn new(device: &wgpu::Device, size: PhysicalSize<u32>, config: PhosphorFormatConfig) -> Self {
        let size = PhysicalSize::new(size.width.max(1), size.height.max(1));
        let history_usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        let scratch_usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;

        let history_a = PhosphorTexture::new(
            device,
            "Phosphor History A",
            size,
            config.format,
            history_usage,
        );
        let history_b = PhosphorTexture::new(
            device,
            "Phosphor History B",
            size,
            config.format,
            history_usage,
        );
        let beam_scratch = PhosphorTexture::new(
            device,
            "Current Beam Scratch",
            size,
            config.format,
            scratch_usage,
        );

        Self {
            config,
            size,
            history: [history_a, history_b],
            beam_scratch,
            previous_index: 0,
            needs_clear: true,
        }
    }

    fn target_index(&self) -> usize {
        1 - self.previous_index
    }

    fn view(&self, index: usize) -> &wgpu::TextureView {
        &self.history[index].view
    }

    fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    fn max_luma(&self) -> f32 {
        self.config.max_luma
    }

    fn needs_clear(&self) -> bool {
        self.needs_clear
    }

    fn mark_clear(&mut self) {
        self.needs_clear = false;
    }

    fn encode_clear(&self, encoder: &mut wgpu::CommandEncoder) {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Asteroids Phosphor History Clear"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: &self.history[0].view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &self.history[1].view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }

    fn copy_target_to_beam_scratch(&self, encoder: &mut wgpu::CommandEncoder, target_index: usize) {
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.history[target_index].texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.beam_scratch.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.size.width,
                height: self.size.height,
                depth_or_array_layers: 1,
            },
        );
    }

    fn advance(&mut self) {
        self.previous_index = self.target_index();
    }
}

struct PhosphorTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl PhosphorTexture {
    fn new(
        device: &wgpu::Device,
        label: &'static str,
        size: PhysicalSize<u32>,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }
}

struct BloomTargets {
    levels: Vec<BloomTexture>,
    accum: Vec<BloomTexture>,
    full: BloomTexture,
}

impl BloomTargets {
    fn new(device: &wgpu::Device, size: PhysicalSize<u32>, format: wgpu::TextureFormat) -> Self {
        let size = PhysicalSize::new(size.width.max(1), size.height.max(1));
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let mut level_size = size;
        let mut levels = Vec::with_capacity(tuning::BLOOM_MIP_LEVELS);
        for level in 0..tuning::BLOOM_MIP_LEVELS {
            level_size = half_size(level_size);
            let label = format!("Bloom Downsample Level {level}");
            levels.push(BloomTexture::new(device, &label, level_size, format, usage));
        }

        let mut accum = Vec::with_capacity(tuning::BLOOM_MIP_LEVELS.saturating_sub(1));
        for (level, source) in levels
            .iter()
            .enumerate()
            .take(tuning::BLOOM_MIP_LEVELS.saturating_sub(1))
        {
            let label = format!("Bloom Upsample Accum Level {level}");
            accum.push(BloomTexture::new(
                device,
                &label,
                source.size,
                format,
                usage,
            ));
        }

        let full = BloomTexture::new(device, "Bloom Full Resolution", size, format, usage);

        Self {
            levels,
            accum,
            full,
        }
    }

    fn level_view(&self, level: usize) -> &wgpu::TextureView {
        &self.levels[level].view
    }

    fn accum_view(&self, level: usize) -> &wgpu::TextureView {
        &self.accum[level].view
    }

    fn full_view(&self) -> &wgpu::TextureView {
        &self.full.view
    }
}

struct BloomTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: PhysicalSize<u32>,
}

impl BloomTexture {
    fn new(
        device: &wgpu::Device,
        label: &str,
        size: PhysicalSize<u32>,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.width.max(1),
                height: size.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            _texture: texture,
            view,
            size: PhysicalSize::new(size.width.max(1), size.height.max(1)),
        }
    }
}

fn half_size(size: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(
        size.width.div_ceil(2).max(1),
        size.height.div_ceil(2).max(1),
    )
}

struct OutputTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl OutputTexture {
    fn new(device: &wgpu::Device, size: PhysicalSize<u32>, format: wgpu::TextureFormat) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Asteroids Headless Output"),
            size: wgpu::Extent3d {
                width: size.width.max(1),
                height: size.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }
}

struct PhosphorBindGroups {
    blend: [wgpu::BindGroup; 2],
    composite: [wgpu::BindGroup; 2],
}

impl PhosphorBindGroups {
    fn new(
        device: &wgpu::Device,
        phosphor: &PhosphorTargets,
        bloom: &BloomTargets,
        blend_pipeline: &PhosphorBlendPipeline,
        composite_pipeline: &CompositePipeline,
    ) -> Self {
        let blend_target_0 = blend_pipeline.create_bind_group(
            device,
            phosphor.view(1),
            &phosphor.beam_scratch.view,
            "Phosphor Blend Bind Group Target 0",
        );
        let blend_target_1 = blend_pipeline.create_bind_group(
            device,
            phosphor.view(0),
            &phosphor.beam_scratch.view,
            "Phosphor Blend Bind Group Target 1",
        );
        let composite_0 = composite_pipeline.create_bind_group(
            device,
            phosphor.view(0),
            bloom.full_view(),
            "Composite Bind Group Phosphor 0",
        );
        let composite_1 = composite_pipeline.create_bind_group(
            device,
            phosphor.view(1),
            bloom.full_view(),
            "Composite Bind Group Phosphor 1",
        );

        Self {
            blend: [blend_target_0, blend_target_1],
            composite: [composite_0, composite_1],
        }
    }

    fn blend(&self, target_index: usize) -> &wgpu::BindGroup {
        &self.blend[target_index]
    }

    fn composite(&self, target_index: usize) -> &wgpu::BindGroup {
        &self.composite[target_index]
    }
}

struct BloomBindGroups {
    down_from_phosphor: [BloomDownsampleBindGroup; 2],
    down_chain: Vec<BloomDownsampleBindGroup>,
    up: Vec<wgpu::BindGroup>,
    final_up: wgpu::BindGroup,
}

impl BloomBindGroups {
    fn new(
        device: &wgpu::Device,
        phosphor: &PhosphorTargets,
        bloom: &BloomTargets,
        pipeline: &BloomPipeline,
    ) -> Self {
        let down_from_phosphor_0 = pipeline.create_downsample_bind_group(
            device,
            phosphor.view(0),
            "Bloom Downsample Bind Group Phosphor 0",
        );
        let down_from_phosphor_1 = pipeline.create_downsample_bind_group(
            device,
            phosphor.view(1),
            "Bloom Downsample Bind Group Phosphor 1",
        );

        let mut down_chain = Vec::with_capacity(tuning::BLOOM_MIP_LEVELS.saturating_sub(1));
        for level in 1..tuning::BLOOM_MIP_LEVELS {
            let label = format!("Bloom Downsample Bind Group Level {level}");
            down_chain.push(pipeline.create_downsample_bind_group(
                device,
                bloom.level_view(level - 1),
                &label,
            ));
        }

        let mut up = Vec::with_capacity(tuning::BLOOM_MIP_LEVELS.saturating_sub(1));
        for target_level in 0..tuning::BLOOM_MIP_LEVELS.saturating_sub(1) {
            let lower_source = if target_level + 1 == tuning::BLOOM_MIP_LEVELS - 1 {
                bloom.level_view(target_level + 1)
            } else {
                bloom.accum_view(target_level + 1)
            };
            let label = format!("Bloom Upsample Bind Group Level {target_level}");
            up.push(pipeline.create_upsample_bind_group(
                device,
                lower_source,
                bloom.level_view(target_level),
                &label,
            ));
        }

        let final_up = pipeline.create_final_upsample_bind_group(
            device,
            bloom.accum_view(0),
            "Bloom Final Upsample Bind Group",
        );

        Self {
            down_from_phosphor: [down_from_phosphor_0, down_from_phosphor_1],
            down_chain,
            up,
            final_up,
        }
    }

    fn update_downsample_thresholds(&self, queue: &wgpu::Queue, threshold: f32) {
        for pass in &self.down_from_phosphor {
            pass.update_threshold(queue, threshold);
        }
        for pass in &self.down_chain {
            pass.update_threshold(queue, 0.0);
        }
    }

    fn down_from_phosphor(&self, target_index: usize) -> &wgpu::BindGroup {
        &self.down_from_phosphor[target_index].bind_group
    }

    fn down_chain(&self, level: usize) -> &wgpu::BindGroup {
        &self.down_chain[level - 1].bind_group
    }
}

struct BloomDownsampleBindGroup {
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
}

impl BloomDownsampleBindGroup {
    fn update_threshold(&self, queue: &wgpu::Queue, threshold: f32) {
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&BloomDownsampleUniforms::new(threshold)),
        );
    }
}

struct BeamLinePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,
    vertex_capacity: usize,
    vertex_count: u32,
    vertices: Vec<BeamVertex>,
}

impl BeamLinePipeline {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Beam Line Shader"),
            source: wgpu::ShaderSource::Wgsl(BEAM_LINE_SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Beam Line Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Beam Line Uniform Buffer"),
            size: size_of::<BeamUniforms>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Beam Line Bind Group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Beam Line Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Beam Line Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[BeamLineVertexLayout::LAYOUT],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(additive_blend_state()),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Beam Line Vertex Buffer"),
            size: 1,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group,
            uniform_buffer,
            vertex_buffer,
            vertex_capacity: 0,
            vertex_count: 0,
            vertices: Vec::new(),
        }
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        commands: &[BeamCommand],
        half_width: f32,
        size: PhysicalSize<u32>,
        max_luma: f32,
    ) {
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&BeamUniforms::new(size, max_luma)),
        );

        beam::expand_beam_commands(commands, half_width, &mut self.vertices);
        self.vertex_count = self.vertices.len() as u32;

        if self.vertices.is_empty() {
            return;
        }

        self.ensure_vertex_capacity(device, self.vertices.len());
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
    }

    fn ensure_vertex_capacity(&mut self, device: &wgpu::Device, required_vertices: usize) {
        if required_vertices <= self.vertex_capacity {
            return;
        }

        let new_capacity = required_vertices.next_power_of_two().max(64);
        self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Beam Line Vertex Buffer"),
            size: (new_capacity * size_of::<BeamVertex>()) as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.vertex_capacity = new_capacity;
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.vertex_count == 0 {
            return;
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}

struct PhosphorBlendPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
}

impl PhosphorBlendPipeline {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Phosphor Blend Shader"),
            source: wgpu::ShaderSource::Wgsl(PHOSPHOR_BLEND_SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Phosphor Blend Bind Group Layout"),
            entries: &[
                texture_bind_group_layout_entry(0, texture_sample_filterable(target_format)),
                texture_bind_group_layout_entry(1, texture_sample_filterable(target_format)),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Phosphor Blend Uniform Buffer"),
            size: size_of::<PhosphorBlendUniforms>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Phosphor Blend Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = fullscreen_pipeline(
            device,
            &pipeline_layout,
            &shader,
            target_format,
            "Phosphor Blend Pipeline",
        );

        Self {
            pipeline,
            bind_group_layout,
            uniform_buffer,
        }
    }

    fn update_uniforms(
        &self,
        queue: &wgpu::Queue,
        frame_dt_seconds: f32,
        tau_seconds: f32,
        max_luma: f32,
    ) {
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&PhosphorBlendUniforms::new(
                frame_dt_seconds,
                tau_seconds,
                max_luma,
            )),
        );
    }

    fn create_bind_group(
        &self,
        device: &wgpu::Device,
        previous_view: &wgpu::TextureView,
        beam_view: &wgpu::TextureView,
        label: &'static str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(previous_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(beam_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        })
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, bind_group: &wgpu::BindGroup) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

struct BloomPipeline {
    downsample_pipeline: wgpu::RenderPipeline,
    upsample_pipeline: wgpu::RenderPipeline,
    final_upsample_pipeline: wgpu::RenderPipeline,
    downsample_bind_group_layout: wgpu::BindGroupLayout,
    upsample_bind_group_layout: wgpu::BindGroupLayout,
    final_upsample_bind_group_layout: wgpu::BindGroupLayout,
}

impl BloomPipeline {
    fn new(device: &wgpu::Device, bloom_format: wgpu::TextureFormat) -> Self {
        let downsample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bloom Downsample Shader"),
            source: wgpu::ShaderSource::Wgsl(BLOOM_DOWNSAMPLE_SHADER.into()),
        });
        let upsample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bloom Upsample Shader"),
            source: wgpu::ShaderSource::Wgsl(BLOOM_UPSAMPLE_SHADER.into()),
        });
        let final_upsample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bloom Final Upsample Shader"),
            source: wgpu::ShaderSource::Wgsl(BLOOM_FINAL_UPSAMPLE_SHADER.into()),
        });
        let downsample_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Bloom Downsample Bind Group Layout"),
                entries: &[
                    texture_bind_group_layout_entry(0, texture_sample_filterable(bloom_format)),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let upsample_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Bloom Upsample Bind Group Layout"),
                entries: &[
                    texture_bind_group_layout_entry(0, texture_sample_filterable(bloom_format)),
                    texture_bind_group_layout_entry(1, texture_sample_filterable(bloom_format)),
                ],
            });
        let final_upsample_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Bloom Final Upsample Bind Group Layout"),
                entries: &[texture_bind_group_layout_entry(
                    0,
                    texture_sample_filterable(bloom_format),
                )],
            });
        let downsample_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Bloom Downsample Pipeline Layout"),
            bind_group_layouts: &[Some(&downsample_bind_group_layout)],
            immediate_size: 0,
        });
        let upsample_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Bloom Upsample Pipeline Layout"),
            bind_group_layouts: &[Some(&upsample_bind_group_layout)],
            immediate_size: 0,
        });
        let final_upsample_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Bloom Final Upsample Pipeline Layout"),
                bind_group_layouts: &[Some(&final_upsample_bind_group_layout)],
                immediate_size: 0,
            });
        let downsample_pipeline = fullscreen_pipeline(
            device,
            &downsample_layout,
            &downsample_shader,
            bloom_format,
            "Bloom Downsample Pipeline",
        );
        let upsample_pipeline = fullscreen_pipeline(
            device,
            &upsample_layout,
            &upsample_shader,
            bloom_format,
            "Bloom Upsample Pipeline",
        );
        let final_upsample_pipeline = fullscreen_pipeline(
            device,
            &final_upsample_layout,
            &final_upsample_shader,
            bloom_format,
            "Bloom Final Upsample Pipeline",
        );

        Self {
            downsample_pipeline,
            upsample_pipeline,
            final_upsample_pipeline,
            downsample_bind_group_layout,
            upsample_bind_group_layout,
            final_upsample_bind_group_layout,
        }
    }

    fn create_downsample_bind_group(
        &self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
        label: &str,
    ) -> BloomDownsampleBindGroup {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bloom Downsample Uniform Buffer"),
            size: size_of::<BloomDownsampleUniforms>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.downsample_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
        });
        BloomDownsampleBindGroup {
            bind_group,
            uniform_buffer,
        }
    }

    fn create_upsample_bind_group(
        &self,
        device: &wgpu::Device,
        lower_source_view: &wgpu::TextureView,
        base_view: &wgpu::TextureView,
        label: &str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.upsample_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(lower_source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(base_view),
                },
            ],
        })
    }

    fn create_final_upsample_bind_group(
        &self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
        label: &str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.final_upsample_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            }],
        })
    }

    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        bloom: &BloomTargets,
        bind_groups: &BloomBindGroups,
        phosphor_index: usize,
    ) {
        self.draw_to_view(
            encoder,
            &self.downsample_pipeline,
            bind_groups.down_from_phosphor(phosphor_index),
            bloom.level_view(0),
            "Asteroids Bloom Downsample Pass 0",
        );

        for level in 1..tuning::BLOOM_MIP_LEVELS {
            self.draw_to_view(
                encoder,
                &self.downsample_pipeline,
                bind_groups.down_chain(level),
                bloom.level_view(level),
                "Asteroids Bloom Downsample Pass",
            );
        }

        for target_level in (0..tuning::BLOOM_MIP_LEVELS.saturating_sub(1)).rev() {
            self.draw_to_view(
                encoder,
                &self.upsample_pipeline,
                &bind_groups.up[target_level],
                bloom.accum_view(target_level),
                "Asteroids Bloom Upsample Accum Pass",
            );
        }

        self.draw_to_view(
            encoder,
            &self.final_upsample_pipeline,
            &bind_groups.final_up,
            bloom.full_view(),
            "Asteroids Bloom Final Upsample Pass",
        );
    }

    fn draw_to_view(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        bind_group: &wgpu::BindGroup,
        target_view: &wgpu::TextureView,
        label: &'static str,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

struct CompositePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
}

impl CompositePipeline {
    fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        phosphor_format: wgpu::TextureFormat,
        bloom_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Phosphor Composite Shader"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Composite Bind Group Layout"),
            entries: &[
                texture_bind_group_layout_entry(0, texture_sample_filterable(phosphor_format)),
                texture_bind_group_layout_entry(1, texture_sample_filterable(bloom_format)),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Composite Uniform Buffer"),
            size: size_of::<CompositeUniforms>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Composite Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = fullscreen_pipeline(
            device,
            &pipeline_layout,
            &shader,
            target_format,
            "Composite Pipeline",
        );

        Self {
            pipeline,
            bind_group_layout,
            uniform_buffer,
        }
    }

    fn update_uniforms(&self, queue: &wgpu::Queue, bloom_intensity: f32) {
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&CompositeUniforms::new(bloom_intensity)),
        );
    }

    fn create_bind_group(
        &self,
        device: &wgpu::Device,
        phosphor_view: &wgpu::TextureView,
        bloom_view: &wgpu::TextureView,
        label: &'static str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(phosphor_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(bloom_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        })
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, bind_group: &wgpu::BindGroup) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

struct BeamLineVertexLayout;

impl BeamLineVertexLayout {
    const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<BeamVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &BEAM_VERTEX_ATTRIBUTES,
    };
}

const BEAM_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x2,
    1 => Float32x2,
    2 => Float32x2,
    3 => Float32,
    4 => Float32,
];

fn texture_bind_group_layout_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn texture_sample_filterable(format: wgpu::TextureFormat) -> bool {
    matches!(
        format,
        wgpu::TextureFormat::Rgba8Unorm
            | wgpu::TextureFormat::Rgba8UnormSrgb
            | wgpu::TextureFormat::Bgra8Unorm
            | wgpu::TextureFormat::Bgra8UnormSrgb
    )
}

fn additive_blend_state() -> wgpu::BlendState {
    let component = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    wgpu::BlendState {
        color: component,
        alpha: component,
    }
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    target_format: wgpu::TextureFormat,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

const BEAM_LINE_SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) segment_start: vec2<f32>,
    @location(2) segment_end: vec2<f32>,
    @location(3) intensity: f32,
    @location(4) dwell_us: f32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) segment_start: vec2<f32>,
    @location(1) segment_end: vec2<f32>,
    @location(2) intensity: f32,
    @location(3) dwell_us: f32,
};

struct BeamUniforms {
    target_size_sigma_dwell: vec4<f32>,
    growth_max_luma_pad: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> beam: BeamUniforms;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(input.position, 0.0, 1.0);
    output.segment_start = input.segment_start;
    output.segment_end = input.segment_end;
    output.intensity = input.intensity;
    output.dwell_us = input.dwell_us;
    return output;
}

fn ndc_to_pixel(ndc: vec2<f32>) -> vec2<f32> {
    let target_size = beam.target_size_sigma_dwell.xy;
    return vec2<f32>(
        (ndc.x * 0.5 + 0.5) * target_size.x,
        (0.5 - ndc.y * 0.5) * target_size.y,
    );
}

fn distance_to_segment_px(point: vec2<f32>, start: vec2<f32>, end: vec2<f32>) -> f32 {
    let segment = end - start;
    let segment_len_sq = max(dot(segment, segment), 0.000001);
    let t = clamp(dot(point - start, segment) / segment_len_sq, 0.0, 1.0);
    let closest = start + segment * t;
    return length(point - closest);
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let start_px = ndc_to_pixel(input.segment_start);
    let end_px = ndc_to_pixel(input.segment_end);
    let distance = distance_to_segment_px(input.position.xy, start_px, end_px);

    let base_sigma = beam.target_size_sigma_dwell.z;
    let dwell_reference = max(beam.target_size_sigma_dwell.w, 0.001);
    let dwell_factor = max(input.dwell_us / dwell_reference, 0.0);
    let sigma_growth = beam.growth_max_luma_pad.x;
    let sigma = base_sigma * (1.0 + sigma_growth * max(dwell_factor - 1.0, 0.0));
    let sigma_sq = max(sigma * sigma, 0.0001);
    let brightness = input.intensity * dwell_factor * exp(-(distance * distance) / sigma_sq);
    let luma = clamp(brightness, 0.0, beam.growth_max_luma_pad.y);

    return vec4<f32>(vec3<f32>(luma), luma);
}
"#;

const PHOSPHOR_BLEND_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

struct PhosphorBlendUniforms {
    frame_dt_tau_max_luma: vec4<f32>,
};

@group(0) @binding(0)
var previous_phosphor: texture_2d<f32>;
@group(0) @binding(1)
var current_beam: texture_2d<f32>;
@group(0) @binding(2)
var<uniform> phosphor: PhosphorBlendUniforms;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(input.position.xy);
    let previous = textureLoad(previous_phosphor, coord, 0).rgb;
    let beam = textureLoad(current_beam, coord, 0).rgb;
    let frame_dt = max(phosphor.frame_dt_tau_max_luma.x, 0.0);
    let tau = max(phosphor.frame_dt_tau_max_luma.y, 0.0001);
    let max_luma = phosphor.frame_dt_tau_max_luma.z;
    let decay = exp(-frame_dt / tau);
    let combined = clamp(beam + previous * decay, vec3<f32>(0.0), vec3<f32>(max_luma));

    return vec4<f32>(combined, 1.0);
}
"#;

const BLOOM_DOWNSAMPLE_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

struct BloomDownsampleUniforms {
    threshold_pad: vec4<f32>,
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;
@group(0) @binding(1)
var<uniform> bloom: BloomDownsampleUniforms;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    return output;
}

fn clamp_coord(coord: vec2<i32>, size: vec2<i32>) -> vec2<i32> {
    return min(max(coord, vec2<i32>(0, 0)), max(size - vec2<i32>(1, 1), vec2<i32>(0, 0)));
}

fn prefilter(color: vec3<f32>) -> vec3<f32> {
    let threshold = max(bloom.threshold_pad.x, 0.0);
    if threshold <= 0.000001 {
        return color;
    }
    let luma = max(max(color.r, color.g), color.b);
    let scale = max(luma - threshold, 0.0) / max(luma, 0.0001);
    return color * scale;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(source_texture));
    let coord = vec2<i32>(input.position.xy) * 2;
    var weights = array<f32, 5>(1.0, 4.0, 6.0, 4.0, 1.0);
    var sum = vec3<f32>(0.0);

    for (var y: u32 = 0u; y < 5u; y = y + 1u) {
        for (var x: u32 = 0u; x < 5u; x = x + 1u) {
            let offset = vec2<i32>(i32(x) - 2, i32(y) - 2);
            let sample_coord = clamp_coord(coord + offset, size);
            let sample = max(textureLoad(source_texture, sample_coord, 0).rgb, vec3<f32>(0.0));
            sum += prefilter(sample) * weights[x] * weights[y];
        }
    }

    return vec4<f32>(sum / 256.0, 1.0);
}
"#;

const BLOOM_UPSAMPLE_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@group(0) @binding(0)
var lower_texture: texture_2d<f32>;
@group(0) @binding(1)
var base_texture: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    return output;
}

fn clamp_coord(coord: vec2<i32>, size: vec2<i32>) -> vec2<i32> {
    return min(max(coord, vec2<i32>(0, 0)), max(size - vec2<i32>(1, 1), vec2<i32>(0, 0)));
}

fn sample_lower_bilinear(pixel: vec2<f32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(lower_texture));
    let base = vec2<i32>(floor(pixel));
    let frac = pixel - floor(pixel);
    let c00 = max(textureLoad(lower_texture, clamp_coord(base, size), 0).rgb, vec3<f32>(0.0));
    let c10 = max(textureLoad(lower_texture, clamp_coord(base + vec2<i32>(1, 0), size), 0).rgb, vec3<f32>(0.0));
    let c01 = max(textureLoad(lower_texture, clamp_coord(base + vec2<i32>(0, 1), size), 0).rgb, vec3<f32>(0.0));
    let c11 = max(textureLoad(lower_texture, clamp_coord(base + vec2<i32>(1, 1), size), 0).rgb, vec3<f32>(0.0));
    return mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);
}

fn sample_lower_gaussian(pixel: vec2<f32>) -> vec3<f32> {
    var weights = array<f32, 3>(1.0, 2.0, 1.0);
    var sum = vec3<f32>(0.0);
    for (var y: u32 = 0u; y < 3u; y = y + 1u) {
        for (var x: u32 = 0u; x < 3u; x = x + 1u) {
            let offset = vec2<f32>(f32(i32(x) - 1), f32(i32(y) - 1));
            sum += sample_lower_bilinear(pixel + offset) * weights[x] * weights[y];
        }
    }
    return sum / 16.0;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let base_size = vec2<i32>(textureDimensions(base_texture));
    let coord = vec2<i32>(input.position.xy);
    let base = max(textureLoad(base_texture, clamp_coord(coord, base_size), 0).rgb, vec3<f32>(0.0));
    let lower_pixel = input.position.xy * 0.5 - vec2<f32>(0.25);
    let glow = sample_lower_gaussian(lower_pixel);
    return vec4<f32>(base + glow, 1.0);
}
"#;

const BLOOM_FINAL_UPSAMPLE_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    return output;
}

fn clamp_coord(coord: vec2<i32>, size: vec2<i32>) -> vec2<i32> {
    return min(max(coord, vec2<i32>(0, 0)), max(size - vec2<i32>(1, 1), vec2<i32>(0, 0)));
}

fn sample_source_bilinear(pixel: vec2<f32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(source_texture));
    let base = vec2<i32>(floor(pixel));
    let frac = pixel - floor(pixel);
    let c00 = max(textureLoad(source_texture, clamp_coord(base, size), 0).rgb, vec3<f32>(0.0));
    let c10 = max(textureLoad(source_texture, clamp_coord(base + vec2<i32>(1, 0), size), 0).rgb, vec3<f32>(0.0));
    let c01 = max(textureLoad(source_texture, clamp_coord(base + vec2<i32>(0, 1), size), 0).rgb, vec3<f32>(0.0));
    let c11 = max(textureLoad(source_texture, clamp_coord(base + vec2<i32>(1, 1), size), 0).rgb, vec3<f32>(0.0));
    return mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);
}

fn sample_source_gaussian(pixel: vec2<f32>) -> vec3<f32> {
    var weights = array<f32, 3>(1.0, 2.0, 1.0);
    var sum = vec3<f32>(0.0);
    for (var y: u32 = 0u; y < 3u; y = y + 1u) {
        for (var x: u32 = 0u; x < 3u; x = x + 1u) {
            let offset = vec2<f32>(f32(i32(x) - 1), f32(i32(y) - 1));
            sum += sample_source_bilinear(pixel + offset) * weights[x] * weights[y];
        }
    }
    return sum / 16.0;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let source_pixel = input.position.xy * 0.5 - vec2<f32>(0.25);
    return vec4<f32>(sample_source_gaussian(source_pixel), 1.0);
}
"#;

const COMPOSITE_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

struct CompositeUniforms {
    bloom_intensity_pad: vec4<f32>,
};

@group(0) @binding(0)
var phosphor_texture: texture_2d<f32>;
@group(0) @binding(1)
var bloom_texture: texture_2d<f32>;
@group(0) @binding(2)
var<uniform> composite: CompositeUniforms;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(input.position.xy);
    let phosphor = max(textureLoad(phosphor_texture, coord, 0).rgb, vec3<f32>(0.0));
    let bloom = max(textureLoad(bloom_texture, coord, 0).rgb, vec3<f32>(0.0));
    let phosphor_luma = max(max(phosphor.r, phosphor.g), phosphor.b);
    let core_guard = 1.0 - smoothstep(0.35, 0.72, phosphor_luma);
    let hdr = phosphor + bloom * composite.bloom_intensity_pad.x * core_guard;
    let reinhard = hdr / (hdr + vec3<f32>(1.0));
    let gamma_encoded = pow(clamp(reinhard, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2));

    return vec4<f32>(gamma_encoded, 1.0);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 0.00001,
            "actual={actual}, expected={expected}"
        );
    }

    #[test]
    fn centered_playfield_uses_side_margins_on_widescreen() {
        let rect = PlayfieldRect::centered_4_3(PhysicalSize::new(1920, 1080));

        assert_close(rect.min.x, -0.75);
        assert_close(rect.max.x, 0.75);
        assert_close(rect.min.y, -1.0);
        assert_close(rect.max.y, 1.0);
        assert!(rect.left_margin().is_some());
        assert!(rect.right_margin().is_some());
    }

    #[test]
    fn playfield_mapping_preserves_corners_inside_centered_rect() {
        let rect = PlayfieldRect::centered_4_3(PhysicalSize::new(5120, 2160));

        assert_eq!(rect.map_point(Vec2::new(-1.0, -1.0)), rect.min);
        assert_eq!(rect.map_point(Vec2::new(1.0, 1.0)), rect.max);
        assert_eq!(rect.map_point(Vec2::ZERO), Vec2::ZERO);
    }
}
