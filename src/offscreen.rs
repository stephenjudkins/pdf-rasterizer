use crate::*;
use eyre::{Result, eyre};
use image::{ImageBuffer, RgbaImage};
use lopdf::Document;
use vello::util::RenderContext;
use vello::{Renderer, RendererOptions, Scene};

pub async fn pdf_to_rgba_image(
    doc: &Document,
    page: u32,
    scale: f32,
    render_settings: &RenderSettings,
) -> Result<RgbaImage> {
    let page_id = doc
        .get_pages()
        .get(&page)
        .ok_or_else(|| eyre!("Page {} not found in PDF", page))?
        .clone();

    let page_dict = doc.get_dictionary(page_id)?;
    let size = dimensions(page_dict)?;

    let width = (size.0 as f32 * scale) as u32;
    let height = (size.1 as f32 * scale) as u32;

    let mut render_cx = RenderContext::new();

    let device_id = render_cx
        .device(None)
        .await
        .ok_or_else(|| eyre!("No compatible device found"))?;

    let mut renderer = Renderer::new(
        &render_cx.devices[device_id].device,
        RendererOptions {
            use_cpu: false,
            antialiasing_support: vello::AaSupport::all(),
            num_init_threads: None,
            pipeline_cache: None,
        },
    )
    .unwrap();

    let device = &render_cx.devices[0].device;
    let queue = &render_cx.devices[0].queue;

    let texture_desc = wgpu::TextureDescriptor {
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::STORAGE_BINDING,
        label: Some("Render Texture"),
        view_formats: &[],
    };
    let texture = device.create_texture(&texture_desc);
    let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let u32_size = std::mem::size_of::<u32>() as u32;
    let unpadded_bytes_per_row = u32_size * width;
    let bytes_per_row = ((unpadded_bytes_per_row + 255) / 256) * 256;
    let output_buffer_size = (bytes_per_row * height) as wgpu::BufferAddress;
    let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        size: output_buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        label: Some("Output Buffer"),
        mapped_at_creation: false,
    });

    let mut scene = Scene::new();

    use kurbo::{Affine, Rect};
    use peniko::Color;
    scene.fill(
        peniko::Fill::NonZero,
        Affine::IDENTITY,
        Color::WHITE,
        None,
        &Rect::new(0.0, 0.0, width as f64, height as f64),
    );

    draw_doc(doc, &mut scene, width, height, page, &render_settings)?;

    let render_params = vello::RenderParams {
        base_color: peniko::Color::BLACK,
        width,
        height,
        antialiasing_method: vello::AaConfig::Msaa16,
    };

    renderer
        .render_to_texture(device, queue, &scene, &texture_view, &render_params)
        .map_err(|e| eyre!("Render error: {:?}", e))?;

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Copy Encoder"),
    });

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            aspect: wgpu::TextureAspect::All,
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &output_buffer,
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

    queue.submit(Some(encoder.finish()));

    let buffer_slice = output_buffer.slice(..);

    let (tx, rx) = std::sync::mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });

    device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap()?;

    let data = buffer_slice.get_mapped_range();

    let image_data = if bytes_per_row != unpadded_bytes_per_row {
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
    output_buffer.unmap();

    Ok(buffer)
}
