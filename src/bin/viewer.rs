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
}

impl AppRenderer {
    fn draw(&mut self, doc: &Document) -> Result<()> {
        let size = self.window.inner_size();

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

        let frame = self
            .surface
            .get_current_texture()
            .wrap_err_with(|| eyre!("unable to get next texture from swapchain"))?;

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let render_params = vello::RenderParams {
            base_color: Color::WHITE,
            width: size.width,
            height: size.height,
            antialiasing_method: AaConfig::Msaa16,
        };

        self.renderer
            .render_to_texture(&self.device, &self.queue, &scene, &view, &render_params)
            .map_err(|e| eyre!("Render error: {:?}", e))?;

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
