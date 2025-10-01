use eyre::{Result, WrapErr, eyre};
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{env, process::ExitCode};

use lopdf::Document;
use rasterizer::*;
use vello::{AaConfig, Renderer, RendererOptions, Scene};
use wgpu::{Device, Queue, Surface};
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::window::{Window, WindowAttributes};
use winit::{application::ApplicationHandler, event_loop::EventLoop};

struct App {
    size: PhysicalSize<u32>,
    doc: Document,
    renderer: Option<Mutex<AppRenderer>>,
}

struct AppRenderer {
    window: Arc<Window>,
    renderer: Renderer,
    surface: Surface<'static>,
    queue: Queue,
    device: Device,
    intermediate_texture: wgpu::Texture,
    intermediate_format: wgpu::TextureFormat,
}

impl AppRenderer {
    fn draw(&mut self, doc: &Document) -> Result<()> {
        let size = self.window.inner_size();

        if self.intermediate_texture.width() != size.width
            || self.intermediate_texture.height() != size.height
        {
            self.intermediate_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                size: wgpu::Extent3d {
                    width: size.width,
                    height: size.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.intermediate_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                label: Some("Intermediate RGBA Texture"),
                view_formats: &[],
            });
        }

        let mut scene = Scene::new();

        use kurbo::{Affine, Rect};
        use peniko::Color;
        scene.fill(
            peniko::Fill::NonZero,
            Affine::IDENTITY,
            Color::WHITE,
            None,
            &Rect::new(0.0, 0.0, size.width as f64, size.height as f64),
        );

        draw_doc(
            doc,
            &mut scene,
            size.width,
            size.height,
            PAGE,
            &RenderSettings::default(),
        )?;

        let intermediate_view = self
            .intermediate_texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let render_params = vello::RenderParams {
            base_color: Color::WHITE,
            width: size.width,
            height: size.height,
            antialiasing_method: AaConfig::Msaa16,
        };

        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                &scene,
                &intermediate_view,
                &render_params,
            )
            .map_err(|e| eyre!("Render error: {:?}", e))?;

        let frame = self
            .surface
            .get_current_texture()
            .wrap_err_with(|| eyre!("unable to get next texture from swapchain"))?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Blit Encoder"),
            });

        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("Blit Bind Group Layout"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });

        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Blit Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blit Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&intermediate_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Blit Shader"),
                source: wgpu::ShaderSource::Wgsl(
                    r#"
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((vertex_index & 1u) << 1u);
    let y = f32((vertex_index & 2u));
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

@group(0) @binding(0) var src_texture: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / vec2<f32>(textureDimensions(src_texture));
    return textureSample(src_texture, src_sampler, uv);
}
"#
                    .into(),
                ),
            });

        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Blit Pipeline Layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Blit Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: frame.texture.format(),
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Blit Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &frame_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(&pipeline);
            render_pass.set_bind_group(0, &bind_group, &[]);
            render_pass.draw(0..3, 0..1);
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();

        Ok(())
    }
}

async fn start(window: Arc<Window>) -> Result<AppRenderer> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .ok_or_else(|| eyre!("failed to get adapter"))?;

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default(), None)
        .await?;

    let size = window.inner_size();

    let surface = instance.create_surface(window.clone())?;
    let mut surface_config = surface
        .get_default_config(&adapter, size.width, size.height)
        .ok_or_else(|| eyre!("failed to get default config"))?;

    let swapchain_capabilities = surface.get_capabilities(&adapter);
    let swapchain_format = swapchain_capabilities
        .formats
        .iter()
        .find(|f| !f.is_srgb())
        .copied()
        .unwrap_or_else(|| swapchain_capabilities.formats[0]);
    surface_config.format = swapchain_format;
    surface.configure(&device, &surface_config);

    let intermediate_format = wgpu::TextureFormat::Rgba8Unorm;
    let intermediate_texture = device.create_texture(&wgpu::TextureDescriptor {
        size: wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: intermediate_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING,
        label: Some("Intermediate RGBA Texture"),
        view_formats: &[],
    });

    let renderer = Renderer::new(
        &device,
        RendererOptions {
            use_cpu: false,
            antialiasing_support: vello::AaSupport::all(),
            num_init_threads: None,
            pipeline_cache: None,
        },
    )
    .map_err(|e| eyre!("Failed to create renderer: {:?}", e))?;

    Ok(AppRenderer {
        window: window,
        renderer: renderer,
        queue: queue,
        surface: surface,
        device: device,
        intermediate_texture,
        intermediate_format,
    })
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let window = event_loop
            .create_window(
                WindowAttributes::default()
                    .with_resizable(false)
                    .with_inner_size(self.size),
            )
            .unwrap();
        let renderer = pollster::block_on(start(Arc::new(window))).unwrap();
        self.renderer = Some(Mutex::new(renderer));
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                let renderer = self.renderer.as_mut().unwrap().get_mut().unwrap();
                renderer.draw(&self.doc).unwrap();
            }
            _ => (),
        }
    }
}

const PAGE: u32 = 1;

const DEFAULT_SCALE: f32 = 2.;

fn go(path: &str, scale: f32) -> Result<()> {
    let bytes = fs::read(path)?;
    let doc = Document::load_mem(&bytes)?;

    let page_id = doc
        .get_pages()
        .get(&PAGE)
        .ok_or_else(|| eyre!("expected page"))?
        .clone();

    let page = doc.get_dictionary(page_id)?;
    let size = dimensions(page)?;

    let event_loop = EventLoop::new()?;

    let mut app = App {
        renderer: None,
        doc: doc,
        size: PhysicalSize {
            width: (size.0 * scale) as u32,
            height: (size.1 * scale) as u32,
        },
    };
    event_loop.run_app(&mut app)?;

    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn main() -> Result<ExitCode> {
    let mut args = env::args().skip(1);
    if let (Some(file), None) = (args.next(), args.next()) {
        go(&file, DEFAULT_SCALE)?;
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("Usage: [filename]");
        Ok(ExitCode::FAILURE)
    }
}
