use crate::*;
use eyre::{Result, eyre};
use femtovg::{Canvas, Color, renderer::WGPURenderer};
use image::{ImageBuffer, RgbaImage};
use lopdf::Document;
use wgpu::{Device, Queue, Texture, TextureView};

pub struct OffscreenRenderer {
    device: Device,
    queue: Queue,
    canvas: Canvas<WGPURenderer>,
    texture: Texture,
    texture_view: TextureView,
    output_buffer: wgpu::Buffer,
    texture_size: (u32, u32),
}

impl OffscreenRenderer {
    pub async fn new(width: u32, height: u32) -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| eyre!("failed to get adapter"))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await?;

        let texture_desc = wgpu::TextureDescriptor {
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::RENDER_ATTACHMENT,
            label: Some("Render Texture"),
            view_formats: &[],
        };
        let texture = device.create_texture(&texture_desc);
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let u32_size = std::mem::size_of::<u32>() as u32;
        // Calculate padded bytes per row for alignment
        let unpadded_bytes_per_row = u32_size * width;
        let bytes_per_row = ((unpadded_bytes_per_row + 255) / 256) * 256;
        let output_buffer_size = (bytes_per_row * height) as wgpu::BufferAddress;
        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            size: output_buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            label: Some("Output Buffer"),
            mapped_at_creation: false,
        });

        let renderer = WGPURenderer::new(device.clone(), queue.clone());
        let canvas = Canvas::new(renderer)?;

        Ok(Self {
            device,
            queue,
            canvas,
            texture,
            texture_view,
            output_buffer,
            texture_size: (width, height),
        })
    }

    pub fn render_pdf(&mut self, doc: &Document, page: u32) -> Result<()> {
        let (width, height) = self.texture_size;

        self.canvas.set_size(width, height, 1.0);
        self.canvas.clear_rect(0, 0, width, height, Color::white());

        draw_doc(doc, &mut self.canvas, page)?;

        let commands = self.canvas.flush_to_surface(&self.texture);
        self.queue.submit(Some(commands));

        Ok(())
    }

    pub async fn to_rgba_image(&self) -> Result<RgbaImage> {
        let (width, height) = self.texture_size;
        let u32_size = std::mem::size_of::<u32>() as u32;

        // Pad bytes_per_row to COPY_BYTES_PER_ROW_ALIGNMENT (256)
        let unpadded_bytes_per_row = u32_size * width;
        let bytes_per_row = ((unpadded_bytes_per_row + 255) / 256) * 256;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Copy Encoder"),
            });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                aspect: wgpu::TextureAspect::All,
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.output_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(Some(encoder.finish()));

        let buffer_slice = self.output_buffer.slice(..);

        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });

        self.device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap()?;

        let data = buffer_slice.get_mapped_range();

        // Extract unpadded data if there's row padding
        let unpadded_bytes_per_row = u32_size * width;
        let bytes_per_row = ((unpadded_bytes_per_row + 255) / 256) * 256;

        let image_data = if bytes_per_row != unpadded_bytes_per_row {
            // Remove padding from each row
            let mut unpadded_data = Vec::with_capacity((unpadded_bytes_per_row * height) as usize);
            for row in 0..height {
                let row_start = (row * bytes_per_row) as usize;
                let row_end = row_start + unpadded_bytes_per_row as usize;
                unpadded_data.extend_from_slice(&data[row_start..row_end]);
            }
            unpadded_data
        } else {
            data.to_vec()
        };

        let buffer: RgbaImage = ImageBuffer::from_raw(width, height, image_data)
            .ok_or_else(|| eyre!("Failed to create image buffer"))?;

        drop(data);
        self.output_buffer.unmap();

        Ok(buffer)
    }
}
