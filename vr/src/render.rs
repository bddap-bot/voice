use ash::vk;
use glam::{Mat4, Vec3};

use crate::vrm::{Alpha, Model, Posed, Vertex};
use crate::vulkan::{vk_error, Gpu};

pub const FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;
const DEPTH: vk::Format = vk::Format::D32_SFLOAT;
const RING: usize = 3;
const NEAR: f32 = 0.01;
const FAR: f32 = 10.0;

const VERTEX_SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vertex.spv"));
const FRAGMENT_SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fragment.spv"));

#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    view_projection: [f32; 16],
    base: [f32; 4],
    shade: [f32; 3],
    blend: f32,
    shading_shift: f32,
    shading_toony: f32,
    cutoff: f32,
    equalization: f32,
}

pub fn eye_projection(eye: Vec3, half_width: f32, half_height: f32) -> Option<Mat4> {
    if eye.z <= NEAR * 2.0 {
        return None;
    }
    let scale = NEAR / eye.z;
    let (left, right) = ((-half_width - eye.x) * scale, (half_width - eye.x) * scale);
    let (bottom, top) = ((-half_height - eye.y) * scale, (half_height - eye.y) * scale);
    let projection = Mat4::from_cols_array_2d(&[
        [2.0 * NEAR / (right - left), 0.0, 0.0, 0.0],
        [0.0, -2.0 * NEAR / (top - bottom), 0.0, 0.0],
        [(right + left) / (right - left), -(top + bottom) / (top - bottom), FAR / (NEAR - FAR), -1.0],
        [0.0, 0.0, NEAR * FAR / (NEAR - FAR), 0.0],
    ]);
    Some(projection * Mat4::from_translation(-eye))
}

struct Texture {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

struct Target {
    texture: Texture,
    framebuffer: vk::Framebuffer,
    commands: vk::CommandBuffer,
    fence: vk::Fence,
}

struct DrawCall {
    first: u32,
    count: u32,
    set: vk::DescriptorSet,
    pipeline: vk::Pipeline,
    push: Push,
}

type Buffer = (vk::Buffer, vk::DeviceMemory, *mut u8);

pub struct Renderer {
    gpu: Gpu,
    pub eye: [u32; 2],
    targets: Vec<Target>,
    depth: Texture,
    render_pass: vk::RenderPass,
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipelines: [[vk::Pipeline; 2]; 2],
    descriptors: vk::DescriptorPool,
    sampler: vk::Sampler,
    textures: Vec<Texture>,
    material_sets: Vec<vk::DescriptorSet>,
    vertices: Option<Buffer>,
    indices: Option<Buffer>,
    drawn: Vec<DrawCall>,
    materials: Vec<crate::vrm::Material>,
    next: usize,
    last: Option<usize>,
}

fn color_range(levels: u32) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(levels).layer_count(1)
}

fn barrier(image: vk::Image, range: vk::ImageSubresourceRange, from: vk::ImageLayout, to: vk::ImageLayout, src: vk::AccessFlags, dst: vk::AccessFlags) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default().old_layout(from).new_layout(to).src_access_mask(src).dst_access_mask(dst).image(image).subresource_range(range)
}

impl Renderer {
    pub fn new(gpu: Gpu, eye: [u32; 2], model: &Model) -> Result<Renderer, String> {
        let device = &gpu.device;
        let attachments = [
            vk::AttachmentDescription::default()
                .format(FORMAT)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL),
            vk::AttachmentDescription::default()
                .format(DEPTH)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
        ];
        let color_reference = [vk::AttachmentReference::default().attachment(0).layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let depth_reference = vk::AttachmentReference::default().attachment(1).layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let subpasses = [vk::SubpassDescription::default().pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS).color_attachments(&color_reference).depth_stencil_attachment(&depth_reference)];
        let render_pass = unsafe { device.create_render_pass(&vk::RenderPassCreateInfo::default().attachments(&attachments).subpasses(&subpasses), None) }.map_err(vk_error("vkCreateRenderPass"))?;

        let bindings = [
            vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(1).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(2).descriptor_type(vk::DescriptorType::SAMPLER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let set_layout = unsafe { device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None) }.map_err(vk_error("vkCreateDescriptorSetLayout"))?;
        let set_layouts = [set_layout];
        let push_ranges = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT).size(std::mem::size_of::<Push>() as u32)];
        let layout = unsafe { device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts).push_constant_ranges(&push_ranges), None) }.map_err(vk_error("vkCreatePipelineLayout"))?;
        let sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::REPEAT)
                    .address_mode_v(vk::SamplerAddressMode::REPEAT)
                    .max_lod(vk::LOD_CLAMP_NONE),
                None,
            )
        }
        .map_err(vk_error("vkCreateSampler"))?;
        let set_count = model.materials.len() as u32;
        let sizes = [vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLED_IMAGE, descriptor_count: 2 * set_count }, vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLER, descriptor_count: set_count }];
        let descriptors = unsafe { device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(set_count).pool_sizes(&sizes), None) }.map_err(vk_error("vkCreateDescriptorPool"))?;

        let mut renderer = Renderer {
            depth: Texture { image: vk::Image::null(), memory: vk::DeviceMemory::null(), view: vk::ImageView::null() },
            gpu,
            eye,
            targets: Vec::new(),
            render_pass,
            set_layout,
            layout,
            pipelines: [[vk::Pipeline::null(); 2]; 2],
            descriptors,
            sampler,
            textures: Vec::new(),
            material_sets: Vec::new(),
            vertices: None,
            indices: None,
            drawn: Vec::new(),
            materials: model.materials.clone(),
            next: 0,
            last: None,
        };
        renderer.pipelines = renderer.pipelines()?;
        renderer.depth = renderer.attachment(DEPTH, vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT, vk::ImageAspectFlags::DEPTH)?;
        for _ in 0..RING {
            let target = renderer.target()?;
            renderer.targets.push(target);
        }
        for image in &model.images {
            let texture = renderer.texture(image.width, image.height, &image.rgba)?;
            renderer.textures.push(texture);
        }
        let white = renderer.textures.len();
        let texture = renderer.texture(1, 1, &[255; 4])?;
        renderer.textures.push(texture);
        let layouts = vec![set_layout; model.materials.len()];
        renderer.material_sets = unsafe { renderer.gpu.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(descriptors).set_layouts(&layouts)) }.map_err(vk_error("vkAllocateDescriptorSets"))?;
        for (material, &set) in model.materials.iter().zip(&renderer.material_sets) {
            let base = [vk::DescriptorImageInfo::default().image_view(renderer.textures[material.base_image.unwrap_or(white)].view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let shade = [vk::DescriptorImageInfo::default().image_view(renderer.textures[material.shade_image.unwrap_or(white)].view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let sampler = [vk::DescriptorImageInfo::default().sampler(renderer.sampler)];
            let writes = [
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&base),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&shade),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(2).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&sampler),
            ];
            unsafe { renderer.gpu.device.update_descriptor_sets(&writes, &[]) };
        }
        Ok(renderer)
    }

    fn pipelines(&self) -> Result<[[vk::Pipeline; 2]; 2], String> {
        let device = &self.gpu.device;
        let module = |code: &[u8]| {
            let words: Vec<u32> = code.chunks_exact(4).map(|word| u32::from_le_bytes(word.try_into().unwrap())).collect();
            unsafe { device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None) }.map_err(vk_error("vkCreateShaderModule"))
        };
        let vertex = module(VERTEX_SHADER)?;
        let fragment = module(FRAGMENT_SHADER)?;
        let stages = [
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vertex).name(c"vertex"),
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(fragment).name(c"fragment"),
        ];
        let vertex_bindings = [vk::VertexInputBindingDescription::default().stride(std::mem::size_of::<Vertex>() as u32).input_rate(vk::VertexInputRate::VERTEX)];
        let vertex_attributes = [
            vk::VertexInputAttributeDescription::default().location(0).format(vk::Format::R32G32B32_SFLOAT).offset(0),
            vk::VertexInputAttributeDescription::default().location(1).format(vk::Format::R32G32B32_SFLOAT).offset(12),
            vk::VertexInputAttributeDescription::default().location(2).format(vk::Format::R32G32_SFLOAT).offset(24),
        ];
        let input = vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&vertex_bindings).vertex_attribute_descriptions(&vertex_attributes);
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let mut pipelines = [[vk::Pipeline::null(); 2]; 2];
        for blend in [false, true] {
            for double_sided in [false, true] {
                let cull = if double_sided { vk::CullModeFlags::NONE } else { vk::CullModeFlags::BACK };
                let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(cull).front_face(vk::FrontFace::COUNTER_CLOCKWISE).line_width(1.0);
                let depth = vk::PipelineDepthStencilStateCreateInfo::default().depth_test_enable(true).depth_write_enable(!blend).depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
                let attachment = [vk::PipelineColorBlendAttachmentState::default()
                    .blend_enable(blend)
                    .src_color_blend_factor(vk::BlendFactor::ONE)
                    .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                    .color_blend_op(vk::BlendOp::ADD)
                    .src_alpha_blend_factor(vk::BlendFactor::ONE)
                    .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                    .alpha_blend_op(vk::BlendOp::ADD)
                    .color_write_mask(vk::ColorComponentFlags::RGBA)];
                let color = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachment);
                let info = vk::GraphicsPipelineCreateInfo::default()
                    .stages(&stages)
                    .vertex_input_state(&input)
                    .input_assembly_state(&assembly)
                    .viewport_state(&viewport)
                    .rasterization_state(&raster)
                    .multisample_state(&multisample)
                    .depth_stencil_state(&depth)
                    .color_blend_state(&color)
                    .dynamic_state(&dynamic)
                    .layout(self.layout)
                    .render_pass(self.render_pass);
                let created = unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) }.map_err(|(_, error)| format!("vkCreateGraphicsPipelines: {error}"))?;
                pipelines[blend as usize][double_sided as usize] = created[0];
            }
        }
        unsafe {
            device.destroy_shader_module(vertex, None);
            device.destroy_shader_module(fragment, None);
        }
        Ok(pipelines)
    }

    fn attachment(&self, format: vk::Format, usage: vk::ImageUsageFlags, aspect: vk::ImageAspectFlags) -> Result<Texture, String> {
        let (image, memory) = self.gpu.image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(format)
                .extent(vk::Extent3D { width: self.eye[0] * 2, height: self.eye[1], depth: 1 })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(usage)
                .initial_layout(vk::ImageLayout::UNDEFINED),
        )?;
        let range = vk::ImageSubresourceRange::default().aspect_mask(aspect).level_count(1).layer_count(1);
        let view = unsafe { self.gpu.device.create_image_view(&vk::ImageViewCreateInfo::default().image(image).view_type(vk::ImageViewType::TYPE_2D).format(format).subresource_range(range), None) }.map_err(vk_error("vkCreateImageView"))?;
        Ok(Texture { image, memory, view })
    }

    fn target(&self) -> Result<Target, String> {
        let device = &self.gpu.device;
        let texture = self.attachment(FORMAT, vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED, vk::ImageAspectFlags::COLOR)?;
        let views = [texture.view, self.depth.view];
        let framebuffer = unsafe { device.create_framebuffer(&vk::FramebufferCreateInfo::default().render_pass(self.render_pass).attachments(&views).width(self.eye[0] * 2).height(self.eye[1]).layers(1), None) }.map_err(vk_error("vkCreateFramebuffer"))?;
        let commands = unsafe { device.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(self.gpu.pool).command_buffer_count(1)) }.map_err(vk_error("vkAllocateCommandBuffers"))?[0];
        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None) }.map_err(vk_error("vkCreateFence"))?;
        Ok(Target { texture, framebuffer, commands, fence })
    }

    fn texture(&self, width: u32, height: u32, rgba: &[u8]) -> Result<Texture, String> {
        let device = &self.gpu.device;
        let levels = 32 - width.max(height).leading_zeros();
        let (image, memory) = self.gpu.image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(FORMAT)
                .extent(vk::Extent3D { width, height, depth: 1 })
                .mip_levels(levels)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED)
                .initial_layout(vk::ImageLayout::UNDEFINED),
        )?;
        let staging = self.gpu.host_buffer(rgba.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC)?;
        unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), staging.2, rgba.len()) };
        self.gpu.once(|commands| unsafe {
            let all = color_range(levels);
            device.cmd_pipeline_barrier(commands, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[barrier(image, all, vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::AccessFlags::empty(), vk::AccessFlags::TRANSFER_WRITE)]);
            let region = vk::BufferImageCopy::default().image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1)).image_extent(vk::Extent3D { width, height, depth: 1 });
            device.cmd_copy_buffer_to_image(commands, staging.0, image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
            let (mut w, mut h) = (width as i32, height as i32);
            for level in 1..levels {
                let source = vk::ImageSubresourceRange { base_mip_level: level - 1, level_count: 1, ..color_range(1) };
                device.cmd_pipeline_barrier(commands, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[barrier(image, source, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::TRANSFER_READ)]);
                let layers = |mip_level| vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level, base_array_layer: 0, layer_count: 1 };
                let (next_w, next_h) = ((w / 2).max(1), (h / 2).max(1));
                let blit = vk::ImageBlit::default()
                    .src_subresource(layers(level - 1))
                    .src_offsets([vk::Offset3D::default(), vk::Offset3D { x: w, y: h, z: 1 }])
                    .dst_subresource(layers(level))
                    .dst_offsets([vk::Offset3D::default(), vk::Offset3D { x: next_w, y: next_h, z: 1 }]);
                device.cmd_blit_image(commands, image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[blit], vk::Filter::LINEAR);
                device.cmd_pipeline_barrier(commands, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER, vk::DependencyFlags::empty(), &[], &[], &[barrier(image, source, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, vk::AccessFlags::TRANSFER_READ, vk::AccessFlags::SHADER_READ)]);
                (w, h) = (next_w, next_h);
            }
            let last = vk::ImageSubresourceRange { base_mip_level: levels - 1, level_count: 1, ..color_range(1) };
            device.cmd_pipeline_barrier(commands, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER, vk::DependencyFlags::empty(), &[], &[], &[barrier(image, last, vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::SHADER_READ)]);
        })?;
        self.gpu.free_buffer(staging);
        let view = unsafe { device.create_image_view(&vk::ImageViewCreateInfo::default().image(image).view_type(vk::ImageViewType::TYPE_2D).format(FORMAT).subresource_range(color_range(levels)), None) }.map_err(vk_error("vkCreateImageView"))?;
        Ok(Texture { image, memory, view })
    }

    pub fn set_mesh(&mut self, posed: &Posed) -> Result<(), String> {
        unsafe { self.gpu.device.device_wait_idle() }.map_err(vk_error("vkDeviceWaitIdle"))?;
        for buffer in [self.vertices.take(), self.indices.take()].into_iter().flatten() {
            self.gpu.free_buffer(buffer);
        }
        let vertex_bytes = std::mem::size_of_val(posed.vertices.as_slice());
        let vertices = self.gpu.host_buffer(vertex_bytes as u64, vk::BufferUsageFlags::VERTEX_BUFFER)?;
        unsafe { std::ptr::copy_nonoverlapping(posed.vertices.as_ptr() as *const u8, vertices.2, vertex_bytes) };
        let index_bytes = std::mem::size_of_val(posed.indices.as_slice());
        let indices = self.gpu.host_buffer(index_bytes as u64, vk::BufferUsageFlags::INDEX_BUFFER)?;
        unsafe { std::ptr::copy_nonoverlapping(posed.indices.as_ptr() as *const u8, indices.2, index_bytes) };
        self.vertices = Some(vertices);
        self.indices = Some(indices);
        self.drawn = posed
            .draws
            .iter()
            .map(|draw| {
                let material = &self.materials[draw.material];
                let blend = material.alpha == Alpha::Blend;
                DrawCall {
                    first: draw.first,
                    count: draw.count,
                    set: self.material_sets[draw.material],
                    pipeline: self.pipelines[blend as usize][material.double_sided as usize],
                    push: Push {
                        view_projection: [0.0; 16],
                        base: material.base,
                        shade: material.shade,
                        blend: blend as u8 as f32,
                        shading_shift: material.shading_shift,
                        shading_toony: material.shading_toony,
                        cutoff: match material.alpha {
                            Alpha::Cutout(cutoff) => cutoff,
                            _ => 0.0,
                        },
                        equalization: material.equalization,
                    },
                }
            })
            .collect();
        Ok(())
    }

    pub fn render(&mut self, eyes: [Option<Mat4>; 2]) -> Result<openvr_sys::VRVulkanTextureData_t, String> {
        let device = &self.gpu.device;
        let target = &self.targets[self.next];
        unsafe { device.wait_for_fences(&[target.fence], true, u64::MAX) }.map_err(vk_error("vkWaitForFences"))?;
        unsafe { device.reset_fences(&[target.fence]) }.map_err(vk_error("vkResetFences"))?;
        let [width, height] = self.eye;
        let clear = [vk::ClearValue { color: vk::ClearColorValue { float32: [0.0; 4] } }, vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } }];
        unsafe {
            device.reset_command_buffer(target.commands, vk::CommandBufferResetFlags::empty()).map_err(vk_error("vkResetCommandBuffer"))?;
            device.begin_command_buffer(target.commands, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).map_err(vk_error("vkBeginCommandBuffer"))?;
            device.cmd_begin_render_pass(
                target.commands,
                &vk::RenderPassBeginInfo::default().render_pass(self.render_pass).framebuffer(target.framebuffer).render_area(vk::Rect2D { offset: vk::Offset2D::default(), extent: vk::Extent2D { width: width * 2, height } }).clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            if let (Some(vertices), Some(indices)) = (&self.vertices, &self.indices) {
                device.cmd_bind_vertex_buffers(target.commands, 0, &[vertices.0], &[0]);
                device.cmd_bind_index_buffer(target.commands, indices.0, 0, vk::IndexType::UINT32);
                for (side, view_projection) in eyes.iter().enumerate() {
                    let Some(view_projection) = view_projection else { continue };
                    let x = (side as u32 * width) as f32;
                    device.cmd_set_viewport(target.commands, 0, &[vk::Viewport { x, y: 0.0, width: width as f32, height: height as f32, min_depth: 0.0, max_depth: 1.0 }]);
                    device.cmd_set_scissor(target.commands, 0, &[vk::Rect2D { offset: vk::Offset2D { x: x as i32, y: 0 }, extent: vk::Extent2D { width, height } }]);
                    for drawn in &self.drawn {
                        let push = Push { view_projection: view_projection.to_cols_array(), ..drawn.push };
                        let bytes = std::slice::from_raw_parts(&push as *const Push as *const u8, std::mem::size_of::<Push>());
                        device.cmd_bind_pipeline(target.commands, vk::PipelineBindPoint::GRAPHICS, drawn.pipeline);
                        device.cmd_bind_descriptor_sets(target.commands, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[drawn.set], &[]);
                        device.cmd_push_constants(target.commands, self.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
                        device.cmd_draw_indexed(target.commands, drawn.count, 1, drawn.first, 0, 0);
                    }
                }
            }
            device.cmd_end_render_pass(target.commands);
            device.end_command_buffer(target.commands).map_err(vk_error("vkEndCommandBuffer"))?;
            let buffers = [target.commands];
            device.queue_submit(self.gpu.queue, &[vk::SubmitInfo::default().command_buffers(&buffers)], target.fence).map_err(vk_error("vkQueueSubmit"))?;
            device.wait_for_fences(&[target.fence], true, u64::MAX).map_err(vk_error("vkWaitForFences"))?;
        }
        self.last = Some(self.next);
        self.next = (self.next + 1) % RING;
        Ok(self.gpu.texture_data(target.texture.image, width * 2, height, FORMAT))
    }

    #[cfg(test)]
    pub fn read(&self) -> Result<Vec<u8>, String> {
        let image = self.targets[self.last.ok_or("nothing rendered")?].texture.image;
        let [width, height] = self.eye;
        let size = (width * 2 * height * 4) as u64;
        let buffer = self.gpu.host_buffer(size, vk::BufferUsageFlags::TRANSFER_DST)?;
        self.gpu.once(|commands| unsafe {
            let region = vk::BufferImageCopy::default().image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1)).image_extent(vk::Extent3D { width: width * 2, height, depth: 1 });
            self.gpu.device.cmd_copy_image_to_buffer(commands, image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, buffer.0, &[region]);
        })?;
        let pixels = unsafe { std::slice::from_raw_parts(buffer.2, size as usize) }.to_vec();
        self.gpu.free_buffer(buffer);
        Ok(pixels)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let device = &self.gpu.device;
        unsafe {
            let _ = device.device_wait_idle();
            for buffer in [self.vertices.take(), self.indices.take()].into_iter().flatten() {
                self.gpu.free_buffer(buffer);
            }
            for target in &self.targets {
                device.destroy_fence(target.fence, None);
                device.destroy_framebuffer(target.framebuffer, None);
            }
            for texture in self.textures.iter().chain(self.targets.iter().map(|target| &target.texture)).chain([&self.depth]) {
                device.destroy_image_view(texture.view, None);
                device.destroy_image(texture.image, None);
                device.free_memory(texture.memory, None);
            }
            for &pipeline in self.pipelines.iter().flatten() {
                device.destroy_pipeline(pipeline, None);
            }
            device.destroy_sampler(self.sampler, None);
            device.destroy_descriptor_pool(self.descriptors, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_render_pass(self.render_pass, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrm::{tests::figure, Version};

    #[test]
    fn the_window_corners_land_on_the_eye_image_corners() {
        let eye = Vec3::new(0.03, 0.1, 0.5);
        let projection = eye_projection(eye, 0.2, 0.2).unwrap();
        for (corner, expected) in [(Vec3::new(-0.2, 0.2, 0.0), [-1.0, -1.0]), (Vec3::new(0.2, -0.2, 0.0), [1.0, 1.0])] {
            let clip = projection * corner.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            assert!((ndc.x - expected[0]).abs() < 1e-5 && (ndc.y - expected[1]).abs() < 1e-5, "{corner} → {ndc}");
            assert!((0.0..1.0).contains(&ndc.z));
        }
        assert!(eye_projection(Vec3::new(0.0, 0.0, 0.01), 0.2, 0.2).is_none(), "an eye at the window draws nothing");
    }

    fn rendered(version: Version, eyes: [Vec3; 2]) -> (Vec<u8>, u32) {
        let model = Model::parse(&figure(version)).unwrap();
        let mut posed = model.pose(&model.rest());
        let fit = posed.fit(0.3);
        posed.place(&fit, version, -0.17);
        let mut renderer = Renderer::new(Gpu::new(None).unwrap(), [64, 64], &model).unwrap();
        renderer.set_mesh(&posed).unwrap();
        renderer.render(eyes.map(|eye| eye_projection(eye, 0.2, 0.2))).unwrap();
        (renderer.read().unwrap(), 128)
    }

    fn pixel(pixels: &[u8], row_length: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * row_length + x) * 4) as usize;
        pixels[at..at + 4].try_into().unwrap()
    }

    fn covered_columns(pixels: &[u8], row_length: u32, eye: u32, y: u32) -> Vec<u32> {
        (0..64).filter(|&x| pixel(pixels, row_length, eye * 64 + x, y)[3] == 255).collect()
    }

    #[test]
    fn both_eyes_draw_the_textured_figure_on_a_clear_background() {
        for version in [Version::Zero, Version::One] {
            let (pixels, row) = rendered(version, [Vec3::new(-0.032, 0.0, 0.4), Vec3::new(0.032, 0.0, 0.4)]);
            for eye in 0..2 {
                assert_eq!(pixel(&pixels, row, eye * 64, 0), [0; 4], "the background stays transparent");
                assert_eq!(pixel(&pixels, row, eye * 64 + 63, 63), [0; 4]);
                let middle = pixel(&pixels, row, eye * 64 + 32, 40);
                assert_eq!(middle[3], 255, "eye {eye}: the opaque cutout figure covers the middle ({version:?})");
                assert!(middle[0] > 0 || middle[1] > 0 || middle[2] > 0);
            }
        }
    }

    #[test]
    fn the_texture_reaches_the_screen_unmixed() {
        let (pixels, row) = rendered(Version::One, [Vec3::new(0.0, 0.0, 0.4); 2]);
        let column = covered_columns(&pixels, row, 0, 58);
        let lower_left = pixel(&pixels, row, column[0] + 2, 58);
        let lower_right = pixel(&pixels, row, column[column.len() / 2 + 2], 58);
        assert!(lower_left[2] > lower_left[0] && lower_left[2] > lower_left[1], "the blue texel is at the figure's lower left: {lower_left:?}");
        assert!(lower_right[0] > lower_left[0] + 60 && lower_right[1] > lower_left[1] + 60, "the white texel is at its lower right: {lower_right:?}");
        assert_eq!(lower_right[3], 255, "a cutout texel above the cutoff is opaque, whatever its alpha");
    }

    #[test]
    fn the_two_eyes_see_the_figure_with_parallax() {
        let (pixels, row) = rendered(Version::One, [Vec3::new(-0.032, 0.0, 0.4), Vec3::new(0.032, 0.0, 0.4)]);
        let [left, right] = [0, 1].map(|eye| covered_columns(&pixels, row, eye, 58));
        assert!(!left.is_empty() && left.len() == right.len());
        assert_eq!(left, right, "geometry on the window plane has no disparity");
        let model = Model::parse(&figure(Version::One)).unwrap();
        let mut posed = model.pose(&model.rest());
        for vertex in &mut posed.vertices {
            vertex.position[2] += 0.5;
        }
        let fit = posed.fit(0.3);
        posed.place(&fit, Version::One, -0.17);
        let mut renderer = Renderer::new(Gpu::new(None).unwrap(), [64, 64], &model).unwrap();
        renderer.set_mesh(&posed).unwrap();
        renderer.render([Vec3::new(-0.032, 0.0, 0.4), Vec3::new(0.032, 0.0, 0.4)].map(|eye| eye_projection(eye, 0.2, 0.2))).unwrap();
        let pixels = renderer.read().unwrap();
        let [left, right] = [0, 1].map(|eye| covered_columns(&pixels, row, eye, 58));
        assert!(left[0] > right[0], "in front of the window the left eye sees it further right: {left:?} {right:?}");
    }
}
