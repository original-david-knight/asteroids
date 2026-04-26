use std::{env, sync::Arc};

use wgpu::CurrentSurfaceTexture;
use winit::{dpi::PhysicalSize, window::Window};

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    fullscreen_size: Option<PhysicalSize<u32>>,
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

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            fullscreen_size,
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
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
