use std::{env, sync::Arc};

use wgpu::CurrentSurfaceTexture;
use winit::{dpi::PhysicalSize, window::Window};

use crate::{
    beam::{self, BeamCommand, BeamEmitter, BeamVertex, Vec2},
    tuning,
};

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    fullscreen_size: Option<PhysicalSize<u32>>,
    beam_pipeline: BeamLinePipeline,
    beam_emitter: BeamEmitter,
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
        let beam_pipeline = BeamLinePipeline::new(&device, format);

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            fullscreen_size,
            beam_pipeline,
            beam_emitter: BeamEmitter::new(),
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
    }

    pub fn render(&mut self) -> Result<(), String> {
        self.beam_emitter.clear();
        emit_demo_beams(&mut self.beam_emitter);
        self.beam_pipeline.upload(
            &self.device,
            &self.queue,
            self.beam_emitter.commands(),
            tuning::BEAM_QUAD_HALF_WIDTH_NDC,
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

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Asteroids Clear Encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Asteroids Beam Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            self.beam_pipeline.draw(&mut pass);
        }

        self.queue.submit([encoder.finish()]);
        frame.present();
        Ok(())
    }

    pub fn size(&self) -> PhysicalSize<u32> {
        self.size
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn present_mode(&self) -> wgpu::PresentMode {
        self.config.present_mode
    }
}

fn emit_demo_beams(emitter: &mut BeamEmitter) {
    emitter
        .emit(
            BeamCommand::builder(Vec2::new(-0.82, -0.82), Vec2::new(0.82, 0.82))
                .intensity(1.0)
                .dwell_us(tuning::SHIP_OUTLINE_SEGMENT_DWELL_US)
                .endpoint_dwell_bonus()
                .build(),
        )
        .emit_segment_with_endpoint_bonus(
            Vec2::new(-0.82, 0.82),
            Vec2::new(0.82, -0.82),
            1.0,
            tuning::SHIP_OUTLINE_SEGMENT_DWELL_US,
        )
        .emit_ship_outline_segment(Vec2::new(-0.35, 0.36), Vec2::new(0.35, 0.36), 0.75)
        .emit_asteroid_hull_segment(Vec2::new(-0.58, 0.0), Vec2::new(0.58, 0.0), 0.9)
        .emit_bullet_dot(Vec2::ZERO, 0.018, 1.0);
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
        wgpu::TextureFormat::Bgra8UnormSrgb,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba8Unorm,
    ]
    .into_iter()
    .find(|format| formats.contains(format))
}

struct BeamLinePipeline {
    pipeline: wgpu::RenderPipeline,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Beam Line Pipeline Layout"),
            bind_group_layouts: &[],
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
                    blend: None,
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
    ) {
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
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
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

const BEAM_LINE_SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) segment_start: vec2<f32>,
    @location(2) segment_end: vec2<f32>,
    @location(3) intensity: f32,
    @location(4) dwell_us: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = vec4<f32>(input.position, 0.0, 1.0);
    return output;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
}
"#;
