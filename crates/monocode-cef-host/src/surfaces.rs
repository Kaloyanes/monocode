//! CEF lends each frame's IOSurface only for the length of its paint
//! callback, so every frame is copied on the GPU into one of a few surfaces
//! this process owns, which the app can keep drawing after the callback.
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_io_surface::{
    IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceHeight, kIOSurfacePixelFormat,
    kIOSurfaceWidth,
};
use objc2_metal::{
    MTLBlitCommandEncoder, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue,
    MTLCreateSystemDefaultDevice, MTLDevice, MTLPixelFormat, MTLTexture, MTLTextureDescriptor,
    MTLTextureUsage,
};
use std::collections::HashMap;

/// Surfaces per browser: the app draws the newest while the next is written,
/// and the third covers a frame still on its way.
const POOL: usize = 3;
const BGRA: i32 = 0x4247_5241; // 'BGRA'

struct Slot {
    surface: CFRetained<IOSurfaceRef>,
    texture: Retained<ProtocolObject<dyn MTLTexture>>,
}

struct Pool {
    size: (usize, usize),
    slots: Vec<Slot>,
    next: usize,
}

pub struct Copier {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pools: HashMap<u64, Pool>,
}

impl Copier {
    pub fn new() -> Option<Self> {
        let device = MTLCreateSystemDefaultDevice()?;
        let queue = device.newCommandQueue()?;
        Some(Self {
            device,
            queue,
            pools: HashMap::new(),
        })
    }

    pub fn forget(&mut self, browser: u64) {
        self.pools.remove(&browser);
    }

    /// Copies `source` into the browser's next surface and returns it.
    pub fn copy(&mut self, browser: u64, source: &IOSurfaceRef) -> Option<&IOSurfaceRef> {
        let size = (source.width(), source.height());
        if size.0 == 0 || size.1 == 0 {
            return None;
        }
        if self.pools.get(&browser).is_none_or(|pool| pool.size != size) {
            let slots = (0..POOL)
                .map(|_| self.slot(size))
                .collect::<Option<Vec<_>>>()?;
            self.pools.insert(
                browser,
                Pool {
                    size,
                    slots,
                    next: 0,
                },
            );
        }
        let source = self.texture(source, size, MTLTextureUsage::ShaderRead)?;
        let pool = self.pools.get_mut(&browser)?;
        let slot = &pool.slots[pool.next];
        pool.next = (pool.next + 1) % POOL;
        let commands = self.queue.commandBuffer()?;
        let blit = commands.blitCommandEncoder()?;
        unsafe { blit.copyFromTexture_toTexture(&source, &slot.texture) };
        blit.endEncoding();
        commands.commit();
        // CEF takes the source back when the callback returns.
        commands.waitUntilCompleted();
        Some(&slot.surface)
    }

    fn slot(&self, (width, height): (usize, usize)) -> Option<Slot> {
        let number = |value: i32| CFNumber::new_i32(value);
        let keys: [&CFString; 4] = unsafe {
            [
                kIOSurfaceWidth,
                kIOSurfaceHeight,
                kIOSurfaceBytesPerElement,
                kIOSurfacePixelFormat,
            ]
        };
        let values = [
            number(width as i32),
            number(height as i32),
            number(4),
            number(BGRA),
        ];
        let values: Vec<&CFType> = values.iter().map(|value| value.as_ref()).collect();
        let properties = CFDictionary::from_slices(&keys, &values);
        let surface = unsafe { IOSurfaceRef::new(properties.as_opaque()) }?;
        let texture = self.texture(&surface, (width, height), MTLTextureUsage::ShaderRead)?;
        Some(Slot { surface, texture })
    }

    fn texture(
        &self,
        surface: &IOSurfaceRef,
        (width, height): (usize, usize),
        usage: MTLTextureUsage,
    ) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::BGRA8Unorm,
                width,
                height,
                false,
            )
        };
        descriptor.setUsage(usage);
        self.device
            .newTextureWithDescriptor_iosurface_plane(&descriptor, surface, 0)
    }
}
