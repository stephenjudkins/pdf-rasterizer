use eyre::{Result, eyre};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{env, process};
use std::{fs::File, io::Read};

use femtovg::Color;
use femtovg::{Canvas, renderer::WGPURenderer};
use lopdf::Document;
use rasterizer::*;
use wgpu::{Queue, Surface};
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
    canvas: Canvas<WGPURenderer>,
    surface: Surface<'static>,
    queue: Queue,
}

impl AppRenderer {
    fn draw(&mut self, doc: &Document) {
        let size = self.window.inner_size();
        let canvas = &mut self.canvas;
        canvas.set_size(size.width, size.height, self.window.scale_factor() as f32);
        canvas.clear_rect(0, 0, size.width, size.height, Color::white());
        draw_doc(doc, canvas, PAGE).unwrap();

        // canvas.fill_text(x, y, text, paint)
        canvas.save();
        canvas.reset();
        let frame = self
            .surface
            .get_current_texture()
            .expect("unable to get next texture from swapchain");
        let commands = canvas.flush_to_surface(&frame.texture);

        self.queue.submit(Some(commands));

        frame.present();

        ()
    }
}

async fn start(window: Arc<Window>) -> AppRenderer {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .unwrap();

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default(), None)
        .await
        .unwrap();

    let size = window.inner_size();

    let surface = instance.create_surface(window.clone()).unwrap();
    let mut surface_config = surface
        .get_default_config(&adapter, size.width, size.height)
        .unwrap();

    let swapchain_capabilities = surface.get_capabilities(&adapter);
    let swapchain_format = swapchain_capabilities
        .formats
        .iter()
        .find(|f| !f.is_srgb())
        .copied()
        .unwrap_or_else(|| swapchain_capabilities.formats[0]);
    surface_config.format = swapchain_format;
    surface.configure(&device, &surface_config);

    let renderer = WGPURenderer::new(device, queue.clone());

    let canvas = Canvas::new(renderer).unwrap();

    AppRenderer {
        window: window,
        canvas: canvas,
        queue: queue,
        surface: surface,
    }
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
        let renderer = pollster::block_on(start(Arc::new(window)));
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
                renderer.draw(&self.doc);
            }
            _ => (),
        }
    }
}

static PAGE: u32 = 1;

static DEFAULT_SCALE: f32 = 2.75;

fn go(path: &String, scale: f32) -> Result<()> {
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let doc = Document::load_mem(&buf)?;

    let page_id = doc
        .get_pages()
        .get(&PAGE)
        .ok_or(eyre!("expected page"))?
        .clone();

    let page = doc.get_dictionary(page_id)?;
    let size = dimensions(page)?;

    let event_loop = EventLoop::new()?;

    let mut app = App {
        renderer: None,
        doc: doc,
        size: PhysicalSize {
            width: size.0 * scale as u32,
            height: size.1 * scale as u32,
        },
    };
    event_loop.run_app(&mut app)?;

    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    match &args[..] {
        [_, file] => {
            go(&file, DEFAULT_SCALE).unwrap();
        }
        _ => {
            eprintln!("Usage: [filename]");
            process::exit(1);
        }
    }
}
