use ash::vk;
use ash::vk::Handle;
use openvr_sys as sys;
use std::ffi::CString;
use std::rc::Rc;

use crate::openvr::Runtime;
use crate::page::Frame;

const FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;
const RING: usize = 3;

struct Slot {
    image: vk::Image,
    image_memory: vk::DeviceMemory,
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    mapped: *mut u8,
    commands: vk::CommandBuffer,
    fence: vk::Fence,
}

pub struct Gpu {
    _entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue: vk::Queue,
    pub family: u32,
    pub pool: vk::CommandPool,
}

pub fn vk_error(context: &str) -> impl Fn(vk::Result) -> String + '_ {
    move |error| format!("{context}: {error}")
}

impl Gpu {
    pub fn new(runtime: Option<&Runtime>) -> Result<Gpu, String> {
        let entry = unsafe { ash::Entry::load() }.map_err(|error| format!("Vulkan loader: {error}"))?;
        let instance_extensions = runtime.map(Runtime::instance_extensions).unwrap_or_default();
        let instance_names: Vec<_> = instance_extensions.iter().map(|name| name.as_ptr()).collect();
        let application = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 1, 0));
        let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&application).enabled_extension_names(&instance_names), None) }.map_err(vk_error("vkCreateInstance"))?;
        let wanted = runtime.map(|runtime| runtime.output_device(instance.handle().as_raw()));
        let physicals = unsafe { instance.enumerate_physical_devices() }.map_err(vk_error("vkEnumeratePhysicalDevices"))?;
        let physical = physicals.iter().copied().find(|device| Some(device.as_raw()) == wanted).or_else(|| physicals.first().copied()).ok_or("no Vulkan device")?;
        let family = unsafe { instance.get_physical_device_queue_family_properties(physical) }
            .iter()
            .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .ok_or("no graphics queue")? as u32;
        let device_extensions: Vec<CString> = runtime.map(|runtime| runtime.device_extensions(physical.as_raw())).unwrap_or_default();
        let device_names: Vec<_> = device_extensions.iter().map(|name| name.as_ptr()).collect();
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&priorities)];
        let device = unsafe { instance.create_device(physical, &vk::DeviceCreateInfo::default().queue_create_infos(&queues).enabled_extension_names(&device_names), None) }.map_err(vk_error("vkCreateDevice"))?;
        let queue = unsafe { device.get_device_queue(family, 0) };
        let pool = unsafe { device.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(family).flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None) }.map_err(vk_error("vkCreateCommandPool"))?;
        Ok(Gpu { _entry: entry, instance, physical, device, queue, family, pool })
    }

    pub fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Result<u32, String> {
        let properties = unsafe { self.instance.get_physical_device_memory_properties(self.physical) };
        (0..properties.memory_type_count)
            .find(|&index| bits & (1 << index) != 0 && properties.memory_types[index as usize].property_flags.contains(flags))
            .ok_or_else(|| format!("no memory type with {flags:?}"))
    }

    pub fn image(&self, info: &vk::ImageCreateInfo) -> Result<(vk::Image, vk::DeviceMemory), String> {
        let image = unsafe { self.device.create_image(info, None) }.map_err(vk_error("vkCreateImage"))?;
        let requirements = unsafe { self.device.get_image_memory_requirements(image) };
        let memory = unsafe { self.device.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(requirements.size).memory_type_index(self.memory_type(requirements.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?), None) }.map_err(vk_error("vkAllocateMemory"))?;
        unsafe { self.device.bind_image_memory(image, memory, 0) }.map_err(vk_error("vkBindImageMemory"))?;
        Ok((image, memory))
    }

    pub fn host_buffer(&self, size: u64, usage: vk::BufferUsageFlags) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8), String> {
        let buffer = unsafe { self.device.create_buffer(&vk::BufferCreateInfo::default().size(size.max(4)).usage(usage), None) }.map_err(vk_error("vkCreateBuffer"))?;
        let requirements = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let host = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        let memory = unsafe { self.device.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(requirements.size).memory_type_index(self.memory_type(requirements.memory_type_bits, host)?), None) }.map_err(vk_error("vkAllocateMemory"))?;
        unsafe { self.device.bind_buffer_memory(buffer, memory, 0) }.map_err(vk_error("vkBindBufferMemory"))?;
        let mapped = unsafe { self.device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) }.map_err(vk_error("vkMapMemory"))? as *mut u8;
        Ok((buffer, memory, mapped))
    }

    pub fn free_buffer(&self, (buffer, memory, _): (vk::Buffer, vk::DeviceMemory, *mut u8)) {
        unsafe {
            self.device.unmap_memory(memory);
            self.device.destroy_buffer(buffer, None);
            self.device.free_memory(memory, None);
        }
    }

    pub fn once(&self, record: impl FnOnce(vk::CommandBuffer)) -> Result<(), String> {
        let commands = unsafe { self.device.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(self.pool).command_buffer_count(1)) }.map_err(vk_error("vkAllocateCommandBuffers"))?[0];
        unsafe {
            self.device.begin_command_buffer(commands, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(vk_error("vkBeginCommandBuffer"))?;
            record(commands);
            self.device.end_command_buffer(commands).map_err(vk_error("vkEndCommandBuffer"))?;
            let buffers = [commands];
            self.device.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&buffers)], vk::Fence::null()).map_err(vk_error("vkQueueSubmit"))?;
            self.device.queue_wait_idle(self.queue).map_err(vk_error("vkQueueWaitIdle"))?;
            self.device.free_command_buffers(self.pool, &buffers);
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn read(&self, image: vk::Image, width: u32, height: u32) -> Result<Vec<u8>, String> {
        let size = (width * height * 4) as u64;
        let buffer = self.host_buffer(size, vk::BufferUsageFlags::TRANSFER_DST)?;
        self.once(|commands| unsafe {
            let region = vk::BufferImageCopy::default().image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1)).image_extent(vk::Extent3D { width, height, depth: 1 });
            self.device.cmd_copy_image_to_buffer(commands, image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, buffer.0, &[region]);
        })?;
        let pixels = unsafe { std::slice::from_raw_parts(buffer.2, size as usize) }.to_vec();
        self.free_buffer(buffer);
        Ok(pixels)
    }

    pub fn texture_data(&self, image: vk::Image, width: u32, height: u32, format: vk::Format) -> sys::VRVulkanTextureData_t {
        sys::VRVulkanTextureData_t {
            m_nImage: image.as_raw(),
            m_pDevice: self.device.handle().as_raw() as *mut sys::VkDevice_T,
            m_pPhysicalDevice: self.physical.as_raw() as *mut sys::VkPhysicalDevice_T,
            m_pInstance: self.instance.handle().as_raw() as *mut sys::VkInstance_T,
            m_pQueue: self.queue.as_raw() as *mut sys::VkQueue_T,
            m_nQueueFamilyIndex: self.family,
            m_nWidth: width,
            m_nHeight: height,
            m_nFormat: format.as_raw() as u32,
            m_nSampleCount: 1,
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// A flat image for an overlay, replaced whole; the compositor may still read the previous ones.
pub struct Flat {
    gpu: Rc<Gpu>,
    images: Vec<(vk::Image, vk::DeviceMemory)>,
    next: usize,
    width: u32,
    height: u32,
}

impl Flat {
    pub fn new(gpu: Rc<Gpu>, width: u32, height: u32) -> Result<Flat, String> {
        let mut images = Vec::new();
        for _ in 0..RING {
            images.push(gpu.image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(FORMAT)
                    .extent(vk::Extent3D { width, height, depth: 1 })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
            )?);
        }
        Ok(Flat { gpu, images, next: 0, width, height })
    }

    pub fn upload(&mut self, rgba: &[u8]) -> Result<sys::VRVulkanTextureData_t, String> {
        assert_eq!(rgba.len(), (self.width * self.height * 4) as usize, "one RGBA texel per pixel");
        let (image, _) = self.images[self.next];
        self.next = (self.next + 1) % RING;
        let staging = self.gpu.host_buffer(rgba.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC)?;
        unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), staging.2, rgba.len()) };
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let barrier = |from, to, src, dst| vk::ImageMemoryBarrier::default().old_layout(from).new_layout(to).src_access_mask(src).dst_access_mask(dst).image(image).subresource_range(range);
        let device = &self.gpu.device;
        let (width, height) = (self.width, self.height);
        let copied = self.gpu.once(|commands| unsafe {
            device.cmd_pipeline_barrier(commands, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[barrier(vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::empty(), vk::AccessFlags::TRANSFER_WRITE)]);
            let region = vk::BufferImageCopy::default().image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1)).image_extent(vk::Extent3D { width, height, depth: 1 });
            device.cmd_copy_buffer_to_image(commands, staging.0, image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
            device.cmd_pipeline_barrier(commands, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::BOTTOM_OF_PIPE, vk::DependencyFlags::empty(), &[], &[], &[barrier(vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::empty())]);
        });
        self.gpu.free_buffer(staging);
        copied?;
        Ok(self.gpu.texture_data(image, width, height, FORMAT))
    }

    #[cfg(test)]
    pub fn read(&self) -> Result<Vec<u8>, String> {
        self.gpu.read(self.images[(self.next + RING - 1) % RING].0, self.width, self.height)
    }
}

impl Drop for Flat {
    fn drop(&mut self) {
        unsafe {
            let _ = self.gpu.device.device_wait_idle();
            for &(image, memory) in &self.images {
                self.gpu.device.destroy_image(image, None);
                self.gpu.device.free_memory(memory, None);
            }
        }
    }
}

pub struct Uploader {
    gpu: Gpu,
    slots: Vec<Slot>,
    next: usize,
    pub width: u32,
    pub height: u32,
}

impl Uploader {
    pub fn new(gpu: Gpu, width: u32, height: u32) -> Result<Uploader, String> {
        let mut uploader = Uploader { gpu, slots: Vec::new(), next: 0, width, height };
        for _ in 0..RING {
            let slot = uploader.slot()?;
            uploader.slots.push(slot);
        }
        Ok(uploader)
    }

    fn slot(&self) -> Result<Slot, String> {
        let device = &self.gpu.device;
        let (image, image_memory) = self.gpu.image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(FORMAT)
                .extent(vk::Extent3D { width: self.width, height: self.height, depth: 1 })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED)
                .initial_layout(vk::ImageLayout::UNDEFINED),
        )?;
        let (staging, staging_memory, mapped) = self.gpu.host_buffer((self.width * self.height * 4) as u64, vk::BufferUsageFlags::TRANSFER_SRC)?;
        let commands = unsafe { device.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(self.gpu.pool).command_buffer_count(1)) }.map_err(vk_error("vkAllocateCommandBuffers"))?[0];
        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None) }.map_err(vk_error("vkCreateFence"))?;
        Ok(Slot { image, image_memory, staging, staging_memory, mapped, commands, fence })
    }

    pub fn upload(&mut self, frame: &Frame) -> Result<sys::VRVulkanTextureData_t, String> {
        let row = (frame.width * 4) as usize;
        let slot = &self.slots[self.next];
        self.next = (self.next + 1) % RING;
        let device = &self.gpu.device;
        unsafe { device.wait_for_fences(&[slot.fence], true, u64::MAX) }.map_err(vk_error("vkWaitForFences"))?;
        unsafe { device.reset_fences(&[slot.fence]) }.map_err(vk_error("vkResetFences"))?;
        let staging = unsafe { std::slice::from_raw_parts_mut(slot.mapped, frame.pixels.len()) };
        let eye_bytes = frame.pixels.len() / 2;
        if row > 0 {
            for (target, source) in staging.chunks_exact_mut(eye_bytes).zip(frame.pixels.chunks_exact(eye_bytes)) {
                for (target, source) in target.chunks_exact_mut(row).zip(source.chunks_exact(row).rev()) {
                    target.copy_from_slice(source);
                }
            }
        }
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let barrier = |from, to, src, dst| vk::ImageMemoryBarrier::default().old_layout(from).new_layout(to).src_access_mask(src).dst_access_mask(dst).image(slot.image).subresource_range(range);
        unsafe {
            device.reset_command_buffer(slot.commands, vk::CommandBufferResetFlags::empty()).map_err(vk_error("vkResetCommandBuffer"))?;
            device.begin_command_buffer(slot.commands, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(vk_error("vkBeginCommandBuffer"))?;
            device.cmd_pipeline_barrier(slot.commands, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[barrier(vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::empty(), vk::AccessFlags::TRANSFER_WRITE)]);
            device.cmd_clear_color_image(slot.commands, slot.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &vk::ClearColorValue { float32: [0.0; 4] }, &[range]);
            if row > 0 {
                device.cmd_pipeline_barrier(slot.commands, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[barrier(vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::TRANSFER_WRITE)]);
                let top = (self.height - frame.y - frame.height) as i32;
                let regions = [0, 1].map(|eye| {
                    vk::BufferImageCopy::default()
                        .buffer_offset((eye * eye_bytes) as u64)
                        .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
                        .image_offset(vk::Offset3D { x: (eye as u32 * self.width / 2 + frame.x) as i32, y: top, z: 0 })
                        .image_extent(vk::Extent3D { width: frame.width, height: frame.height, depth: 1 })
                });
                device.cmd_copy_buffer_to_image(slot.commands, slot.staging, slot.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &regions);
            }
            device.cmd_pipeline_barrier(slot.commands, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::BOTTOM_OF_PIPE, vk::DependencyFlags::empty(), &[], &[], &[barrier(vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::empty())]);
            device.end_command_buffer(slot.commands).map_err(vk_error("vkEndCommandBuffer"))?;
            let buffers = [slot.commands];
            device.queue_submit(self.gpu.queue, &[vk::SubmitInfo::default().command_buffers(&buffers)], slot.fence).map_err(vk_error("vkQueueSubmit"))?;
            device.wait_for_fences(&[slot.fence], true, u64::MAX).map_err(vk_error("vkWaitForFences"))?;
        }
        Ok(self.gpu.texture_data(slot.image, self.width, self.height, FORMAT))
    }
}

impl Drop for Uploader {
    fn drop(&mut self) {
        let device = &self.gpu.device;
        unsafe {
            let _ = device.device_wait_idle();
            for slot in &self.slots {
                device.destroy_fence(slot.fence, None);
                self.gpu.free_buffer((slot.staging, slot.staging_memory, slot.mapped));
                device.destroy_image(slot.image, None);
                device.free_memory(slot.image_memory, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_image_reaches_the_overlay_texture_as_given() {
        let mut flat = Flat::new(Rc::new(Gpu::new(None).unwrap()), 3, 2).unwrap();
        for round in 0..RING as u8 + 1 {
            let rgba: Vec<u8> = (0..24).map(|byte| byte * 10 + round).collect();
            let texture = flat.upload(&rgba).unwrap();
            assert_eq!((texture.m_nWidth, texture.m_nHeight), (3, 2));
            assert_eq!(flat.read().unwrap(), rgba, "round {round}");
        }
    }
}
