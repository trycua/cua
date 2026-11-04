// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Direct VideoToolbox surface presentation for the macOS native client.
//!
//! The decoder yields an IOSurface-backed `CVPixelBuffer`. This module maps
//! that surface into Metal, imports the exact `MTLTexture` into wgpu, and
//! samples it into the window surface. No decoded pixels visit a CPU buffer.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_metal_v3::{MTLPixelFormat, MTLTexture, MTLTextureType};
use objc2_v6::rc::Retained;
use objc2_v6::runtime::ProtocolObject;
use pixels::wgpu;

use super::h264::DecodedPixelBuffer;
use super::Viewport;

const RETIRED_FRAME_COUNT: usize = 3;
const CACHE_FLUSH_INTERVAL: u64 = 120;

const FRAME_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coord: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let position = positions[index];
    var output: VertexOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.tex_coord = position * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return output;
}

@group(0) @binding(0) var frame_texture: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(frame_texture, frame_sampler, input.tex_coord);
}
"#;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: *const c_void);
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVMetalTextureCacheCreate(
        allocator: *const c_void,
        cache_attributes: *const c_void,
        metal_device: *const c_void,
        texture_attributes: *const c_void,
        cache_out: *mut *mut c_void,
    ) -> i32;
    fn CVMetalTextureCacheCreateTextureFromImage(
        allocator: *const c_void,
        texture_cache: *mut c_void,
        source_image: *mut c_void,
        texture_attributes: *const c_void,
        pixel_format: usize,
        width: usize,
        height: usize,
        plane_index: usize,
        texture_out: *mut *mut c_void,
    ) -> i32;
    fn CVMetalTextureGetTexture(image: *mut c_void) -> *mut c_void;
    fn CVMetalTextureCacheFlush(texture_cache: *mut c_void, options: u64);
}

struct OwnedCf(NonNull<c_void>);

impl Drop for OwnedCf {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0.as_ptr()) };
    }
}

struct MetalTextureCache(OwnedCf);

impl MetalTextureCache {
    fn new(device: &wgpu::Device) -> Result<Self, String> {
        let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }
            .ok_or_else(|| "the macOS renderer is not using the Metal backend".to_owned())?;
        let raw_device = Retained::as_ptr(hal_device.raw_device()).cast::<c_void>();
        let mut cache = std::ptr::null_mut();
        let status = unsafe {
            CVMetalTextureCacheCreate(
                std::ptr::null(),
                std::ptr::null(),
                raw_device,
                std::ptr::null(),
                &mut cache,
            )
        };
        let cache = NonNull::new(cache);
        if status != 0 || cache.is_none() {
            return Err(format!("CVMetalTextureCacheCreate returned {status}"));
        }
        Ok(Self(OwnedCf(cache.expect("cache was checked above"))))
    }

    fn flush(&self) {
        unsafe { CVMetalTextureCacheFlush(self.0 .0.as_ptr(), 0) };
    }
}

struct PreparedMetalFrame {
    _cv_texture: OwnedCf,
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

pub(super) struct MetalFrameRenderer {
    texture_cache: MetalTextureCache,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    current: Option<PreparedMetalFrame>,
    retired: VecDeque<PreparedMetalFrame>,
    imported_frames: u64,
}

impl MetalFrameRenderer {
    pub(super) fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
    ) -> Result<Self, String> {
        let texture_cache = MetalTextureCache::new(device)?;
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rcdp_macos_frame_bind_group_layout"),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rcdp_macos_frame_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rcdp_macos_frame_shader"),
            source: wgpu::ShaderSource::Wgsl(FRAME_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rcdp_macos_frame_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rcdp_macos_frame_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        Ok(Self {
            texture_cache,
            bind_group_layout,
            sampler,
            pipeline,
            current: None,
            retired: VecDeque::with_capacity(RETIRED_FRAME_COUNT),
            imported_frames: 0,
        })
    }

    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        pixel_buffer: &DecodedPixelBuffer,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let width_usize = usize::try_from(width).map_err(|_| "frame width exceeds usize")?;
        let height_usize = usize::try_from(height).map_err(|_| "frame height exceeds usize")?;
        let mut cv_texture = std::ptr::null_mut();
        let status = unsafe {
            CVMetalTextureCacheCreateTextureFromImage(
                std::ptr::null(),
                self.texture_cache.0 .0.as_ptr(),
                pixel_buffer.as_ptr(),
                std::ptr::null(),
                MTLPixelFormat::BGRA8Unorm_sRGB.0,
                width_usize,
                height_usize,
                0,
                &mut cv_texture,
            )
        };
        let cv_texture = NonNull::new(cv_texture);
        if status != 0 || cv_texture.is_none() {
            return Err(format!(
                "CVMetalTextureCacheCreateTextureFromImage returned {status}"
            ));
        }
        let cv_texture = OwnedCf(cv_texture.expect("texture was checked above"));
        let metal_texture = unsafe { CVMetalTextureGetTexture(cv_texture.0.as_ptr()) };
        let metal_texture = NonNull::new(metal_texture)
            .ok_or_else(|| "CVMetalTextureGetTexture returned null".to_owned())?;
        let metal_texture = metal_texture.cast::<ProtocolObject<dyn MTLTexture>>();
        let metal_texture = unsafe { Retained::retain(metal_texture.as_ptr()) }
            .ok_or_else(|| "retaining the mapped MTLTexture failed".to_owned())?;

        let format = wgpu::TextureFormat::Bgra8UnormSrgb;
        let hal_texture = unsafe {
            wgpu::hal::metal::Device::texture_from_raw(
                metal_texture,
                format,
                MTLTextureType::Type2D,
                1,
                1,
                wgpu::hal::CopyExtent {
                    width,
                    height,
                    depth: 1,
                },
            )
        };
        let descriptor = wgpu::TextureDescriptor {
            label: Some("rcdp_videotoolbox_frame"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        let texture = unsafe {
            device.create_texture_from_hal::<wgpu::hal::api::Metal>(hal_texture, &descriptor)
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rcdp_macos_frame_bind_group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        if let Some(previous) = self.current.replace(PreparedMetalFrame {
            _cv_texture: cv_texture,
            _texture: texture,
            bind_group,
        }) {
            self.retired.push_back(previous);
        }
        while self.retired.len() > RETIRED_FRAME_COUNT {
            self.retired.pop_front();
        }
        self.imported_frames = self.imported_frames.saturating_add(1);
        if self.imported_frames.is_multiple_of(CACHE_FLUSH_INTERVAL) {
            self.texture_cache.flush();
        }
        Ok(())
    }

    pub(super) fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: Viewport,
    ) {
        let Some(frame) = self.current.as_ref() else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rcdp_macos_frame_render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0627,
                        g: 0.0667,
                        b: 0.0784,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &frame.bind_group, &[]);
        pass.set_viewport(
            viewport.left as f32,
            viewport.top as f32,
            viewport.width as f32,
            viewport.height as f32,
            0.0,
            1.0,
        );
        pass.draw(0..3, 0..1);
    }
}
