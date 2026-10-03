use ash::vk;
use ash::vk::Handle;
use openvr_sys as sys;
use std::ffi::CString;

use crate::openvr::Runtime;

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

pub struct Uploader {
    _entry: ash::Entry,
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    family: u32,
    pool: vk::CommandPool,
    slots: Vec<Slot>,
    next: usize,
    pub width: u32,
    pub height: u32,
}

fn vk_error(context: &str) -> impl Fn(vk::Result) -> String + '_ {
    move |error| format!("{context}: {error}")
}

impl Uploader {
    pub fn new(runtime: &Runtime, width: u32, height: u32) -> Result<Uploader, String> {
        let entry = unsafe { ash::Entry::load() }.map_err(|error| format!("Vulkan loader: {error}"))?;
        let instance_extensions = runtime.instance_extensions();
        let instance_names: Vec<_> = instance_extensions.iter().map(|name| name.as_ptr()).collect();
        let application = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 1, 0));
        let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&application).enabled_extension_names(&instance_names), None) }.map_err(vk_error("vkCreateInstance"))?;
        let wanted = runtime.output_device(instance.handle().as_raw());
        let physicals = unsafe { instance.enumerate_physical_devices() }.map_err(vk_error("vkEnumeratePhysicalDevices"))?;
        let physical = physicals.iter().copied().find(|device| device.as_raw() == wanted).or_else(|| physicals.first().copied()).ok_or("no Vulkan device")?;
        let family = unsafe { instance.get_physical_device_queue_family_properties(physical) }
            .iter()
            .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .ok_or("no graphics queue")? as u32;
        let device_extensions: Vec<CString> = runtime.device_extensions(physical.as_raw());
        let device_names: Vec<_> = device_extensions.iter().map(|name| name.as_ptr()).collect();
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&priorities)];
        let device = unsafe { instance.create_device(physical, &vk::DeviceCreateInfo::default().queue_create_infos(&queues).enabled_extension_names(&device_names), None) }.map_err(vk_error("vkCreateDevice"))?;
        let queue = unsafe { device.get_device_queue(family, 0) };
        let pool = unsafe { device.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(family).flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None) }.map_err(vk_error("vkCreateCommandPool"))?;
        let mut uploader = Uploader { _entry: entry, instance, physical, device, queue, family, pool, slots: Vec::new(), next: 0, width, height };
        for _ in 0..RING {
            let slot = uploader.slot()?;
            uploader.slots.push(slot);
        }
        Ok(uploader)
    }

    fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Result<u32, String> {
        let properties = unsafe { self.instance.get_physical_device_memory_properties(self.physical) };
        (0..properties.memory_type_count)
            .find(|&index| bits & (1 << index) != 0 && properties.memory_types[index as usize].property_flags.contains(flags))
            .ok_or_else(|| format!("no memory type with {flags:?}"))
    }

    fn slot(&self) -> Result<Slot, String> {
        let device = &self.device;
        let image = unsafe {
            device.create_image(
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
                None,
            )
        }
        .map_err(vk_error("vkCreateImage"))?;
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let image_memory = unsafe { device.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(requirements.size).memory_type_index(self.memory_type(requirements.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?), None) }.map_err(vk_error("vkAllocateMemory"))?;
        unsafe { device.bind_image_memory(image, image_memory, 0) }.map_err(vk_error("vkBindImageMemory"))?;
        let size = (self.width * self.height * 4) as u64;
        let staging = unsafe { device.create_buffer(&vk::BufferCreateInfo::default().size(size).usage(vk::BufferUsageFlags::TRANSFER_SRC), None) }.map_err(vk_error("vkCreateBuffer"))?;
        let requirements = unsafe { device.get_buffer_memory_requirements(staging) };
        let host = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        let staging_memory = unsafe { device.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(requirements.size).memory_type_index(self.memory_type(requirements.memory_type_bits, host)?), None) }.map_err(vk_error("vkAllocateMemory"))?;
        unsafe { device.bind_buffer_memory(staging, staging_memory, 0) }.map_err(vk_error("vkBindBufferMemory"))?;
        let mapped = unsafe { device.map_memory(staging_memory, 0, size, vk::MemoryMapFlags::empty()) }.map_err(vk_error("vkMapMemory"))? as *mut u8;
        let commands = unsafe { device.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(self.pool).command_buffer_count(1)) }.map_err(vk_error("vkAllocateCommandBuffers"))?[0];
        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None) }.map_err(vk_error("vkCreateFence"))?;
        Ok(Slot { image, image_memory, staging, staging_memory, mapped, commands, fence })
    }

    pub fn upload(&mut self, bottom_up: &[u8]) -> Result<sys::VRVulkanTextureData_t, String> {
        let row = (self.width * 4) as usize;
        if bottom_up.len() != row * self.height as usize {
            return Err(format!("frame of {} bytes, expected {}", bottom_up.len(), row * self.height as usize));
        }
        let slot = &self.slots[self.next];
        self.next = (self.next + 1) % RING;
        let device = &self.device;
        unsafe { device.wait_for_fences(&[slot.fence], true, u64::MAX) }.map_err(vk_error("vkWaitForFences"))?;
        unsafe { device.reset_fences(&[slot.fence]) }.map_err(vk_error("vkResetFences"))?;
        let staging = unsafe { std::slice::from_raw_parts_mut(slot.mapped, bottom_up.len()) };
        for (target, source) in staging.chunks_exact_mut(row).zip(bottom_up.chunks_exact(row).rev()) {
            target.copy_from_slice(source);
        }
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let barrier = |from, to, src, dst| vk::ImageMemoryBarrier::default().old_layout(from).new_layout(to).src_access_mask(src).dst_access_mask(dst).image(slot.image).subresource_range(range);
        unsafe {
            device.reset_command_buffer(slot.commands, vk::CommandBufferResetFlags::empty()).map_err(vk_error("vkResetCommandBuffer"))?;
            device.begin_command_buffer(slot.commands, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(vk_error("vkBeginCommandBuffer"))?;
            device.cmd_pipeline_barrier(slot.commands, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[barrier(vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::empty(), vk::AccessFlags::TRANSFER_WRITE)]);
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
                .image_extent(vk::Extent3D { width: self.width, height: self.height, depth: 1 });
            device.cmd_copy_buffer_to_image(slot.commands, slot.staging, slot.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
            device.cmd_pipeline_barrier(slot.commands, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::BOTTOM_OF_PIPE, vk::DependencyFlags::empty(), &[], &[], &[barrier(vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::empty())]);
            device.end_command_buffer(slot.commands).map_err(vk_error("vkEndCommandBuffer"))?;
            let buffers = [slot.commands];
            device.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&buffers)], slot.fence).map_err(vk_error("vkQueueSubmit"))?;
            device.wait_for_fences(&[slot.fence], true, u64::MAX).map_err(vk_error("vkWaitForFences"))?;
        }
        Ok(sys::VRVulkanTextureData_t {
            m_nImage: slot.image.as_raw(),
            m_pDevice: self.device.handle().as_raw() as *mut sys::VkDevice_T,
            m_pPhysicalDevice: self.physical.as_raw() as *mut sys::VkPhysicalDevice_T,
            m_pInstance: self.instance.handle().as_raw() as *mut sys::VkInstance_T,
            m_pQueue: self.queue.as_raw() as *mut sys::VkQueue_T,
            m_nQueueFamilyIndex: self.family,
            m_nWidth: self.width,
            m_nHeight: self.height,
            m_nFormat: FORMAT.as_raw() as u32,
            m_nSampleCount: 1,
        })
    }
}

impl Drop for Uploader {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            for slot in &self.slots {
                self.device.destroy_fence(slot.fence, None);
                self.device.unmap_memory(slot.staging_memory);
                self.device.destroy_buffer(slot.staging, None);
                self.device.free_memory(slot.staging_memory, None);
                self.device.destroy_image(slot.image, None);
                self.device.free_memory(slot.image_memory, None);
            }
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}
