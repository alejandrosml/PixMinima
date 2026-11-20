use std::sync::{Mutex, OnceLock, mpsc};
use wgpu::{BufferDescriptor, BufferUsages as Usage, ComputePipeline, Device, Queue};

struct Metal {
    device: Device,
    queue: Queue,
    pipeline: ComputePipeline,
    buffers: Option<Buffers>,
}

struct Buffers {
    pixels: wgpu::Buffer,
    selected: wgpu::Buffer,
    dimensions: wgpu::Buffer,
    bindings: wgpu::BindGroup,
    shape: Option<(usize, usize)>,
}

static CONTEXT: OnceLock<Option<Mutex<Metal>>> = OnceLock::new();
static SHARED: OnceLock<(Device, Queue)> = OnceLock::new();

/// Borrow the renderer's Metal context when it was created with shared-buffer support.
pub fn share_metal_device(device: &Device, queue: &Queue) {
    if device.adapter_info().backend == wgpu::Backend::Metal
        && device
            .features()
            .contains(wgpu::Features::MAPPABLE_PRIMARY_BUFFERS)
    {
        let _ = SHARED.set((device.clone(), queue.clone()));
    }
}

impl Metal {
    fn new() -> Option<Self> {
        let (device, queue) = if let Some(shared) = SHARED.get() {
            shared.clone()
        } else {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::METAL,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: wgpu::Features::MAPPABLE_PRIMARY_BUFFERS,
                ..Default::default()
            }))
            .ok()?
        };
        let guard = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("PixMinima PNG predictors"),
            source: wgpu::ShaderSource::Wgsl(include_str!("rows.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Choose reversible filters"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        if pollster::block_on(guard.pop()).is_some() {
            return None;
        }
        Some(Self {
            device,
            queue,
            pipeline,
            buffers: None,
        })
    }
    fn buffers(&self, bytes: usize, rows: usize) -> Buffers {
        let make = |label, size, usage, mapped_at_creation| {
            self.device.create_buffer(&BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation,
            })
        };
        let pixels = make(
            "RGBA8 pixels",
            bytes as u64,
            Usage::STORAGE | Usage::MAP_WRITE,
            true,
        );
        let selected = make(
            "PNG row choices",
            (rows * 4) as u64,
            Usage::STORAGE | Usage::MAP_READ,
            false,
        );
        let dimensions = make(
            "Image dimensions",
            16,
            Usage::UNIFORM | Usage::COPY_DST,
            false,
        );
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: pixels.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: selected.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: dimensions.as_entire_binding(),
                },
            ],
        });
        Buffers {
            pixels,
            selected,
            dimensions,
            bindings,
            shape: None,
        }
    }
    fn rows(&mut self, input: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
        let limits = self.device.limits();
        if input.is_empty()
            || input.len() > limits.max_storage_buffer_binding_size as usize
            || width > 32_768
            || height > limits.max_compute_workgroups_per_dimension as usize
        {
            return None;
        }
        // Keep shared GPU storage mapped for the next upload, and reuse the same
        // bind group. Both remaps finish in a single device wait below.
        let (capacity, rows) = self.buffers.as_ref().map_or((input.len(), height), |b| {
            (
                input.len().max(b.pixels.size() as usize),
                height.max(b.selected.size() as usize / 4),
            )
        });
        let mut buffers = self
            .buffers
            .take()
            .filter(|b| {
                b.pixels.size() >= input.len() as u64 && b.selected.size() >= (height * 4) as u64
            })
            .unwrap_or_else(|| self.buffers(capacity, rows));
        buffers
            .pixels
            .get_mapped_range_mut(..input.len() as u64)
            .ok()?
            .copy_from_slice(input);
        buffers.pixels.unmap();
        if buffers.shape != Some((width, height)) {
            let mut dims = [0; 16];
            dims[..4].copy_from_slice(&(width as u32).to_le_bytes());
            dims[4..8].copy_from_slice(&(height as u32).to_le_bytes());
            self.queue.write_buffer(&buffers.dimensions, 0, &dims);
            buffers.shape = Some((width, height));
        }
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &buffers.bindings, &[]);
            pass.dispatch_workgroups(height as u32, 1, 1);
        }
        let submission = self.queue.submit([enc.finish()]);
        let (tx, rx) = mpsc::sync_channel(2);
        let upload_ready = tx.clone();
        buffers
            .pixels
            .map_async(wgpu::MapMode::Write, .., move |m| {
                let _ = upload_ready.send(m);
            });
        let result_size = (height * 4) as u64;
        buffers
            .selected
            .map_async(wgpu::MapMode::Read, ..result_size, move |m| {
                let _ = tx.send(m);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(5)),
            })
            .ok()?;
        for _ in 0..2 {
            rx.try_recv().ok()?.ok()?;
        }
        let mapped = buffers.selected.get_mapped_range(..result_size).ok()?;
        let rows: Option<Vec<u8>> = mapped
            .as_chunks::<4>()
            .0
            .iter()
            .map(|v| {
                let kind = u32::from_le_bytes(*v);
                (kind < 5).then_some(kind as u8)
            })
            .collect();
        drop(mapped);
        buffers.selected.unmap();
        self.buffers = Some(buffers);
        rows
    }
}

fn context() -> Option<&'static Mutex<Metal>> {
    CONTEXT
        .get_or_init(|| Metal::new().map(Mutex::new))
        .as_ref()
}
pub(super) fn ready() -> bool {
    context().is_some()
}
pub(super) fn try_choose(input: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    context()?.try_lock().ok()?.rows(input, width, height)
}

pub(super) fn choose(input: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    context()?.lock().ok()?.rows(input, width, height)
}
