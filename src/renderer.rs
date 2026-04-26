use std::{env, sync::Arc};

use bytemuck::{Pod, Zeroable};
use wgpu::CurrentSurfaceTexture;
use wgpu::util::DeviceExt;
use winit::{dpi::PhysicalSize, window::Window};

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    fullscreen_size: Option<PhysicalSize<u32>>,
    hello_triangle: HelloTriangle,
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
        let hello_triangle = HelloTriangle::new(&device, format);

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            fullscreen_size,
            hello_triangle,
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
                label: Some("Asteroids Black Clear"),
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
            self.hello_triangle.draw(&mut pass);
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

struct HelloTriangle {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    vertex_count: u32,
}

impl HelloTriangle {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Hello Triangle Shader"),
            source: wgpu::ShaderSource::Wgsl(HELLO_TRIANGLE_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Hello Triangle Pipeline Layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Hello Triangle Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[TriangleVertex::LAYOUT],
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
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Hello Triangle Vertex Buffer"),
            contents: bytemuck::cast_slice(HELLO_TRIANGLE_VERTICES),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Self {
            pipeline,
            vertex_buffer,
            vertex_count: HELLO_TRIANGLE_VERTICES.len() as u32,
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TriangleVertex {
    position: [f32; 2],
    color: [f32; 3],
}

impl TriangleVertex {
    const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<Self>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &TRIANGLE_VERTEX_ATTRIBUTES,
    };
}

const TRIANGLE_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
    0 => Float32x2,
    1 => Float32x3,
];

const HELLO_TRIANGLE_VERTICES: &[TriangleVertex] = &[
    TriangleVertex {
        position: [0.0, 0.72],
        color: [1.0, 0.95, 0.72],
    },
    TriangleVertex {
        position: [-0.72, -0.62],
        color: [0.1, 0.9, 1.0],
    },
    TriangleVertex {
        position: [0.72, -0.62],
        color: [1.0, 0.2, 0.45],
    },
];

const HELLO_TRIANGLE_SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = vec4<f32>(input.position, 0.0, 1.0);
    output.color = input.color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(input.color, 1.0);
}
"#;
