use ash::vk;
use glam::{Mat4, Vec3};

use std::collections::HashMap;
use std::rc::Rc;

use crate::vrm::{Alpha, Model, OutlineWidth, Skinned, Vertex};
use crate::vulkan::{vk_error, Gpu};

pub const FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;
const DEPTH: vk::Format = vk::Format::D32_SFLOAT;
const RING: usize = 3;
const NEAR: f32 = 0.01;
pub const PAGE_HEIGHT: f32 = 2.7;
const FAR: f32 = 10.0;

const VERTEX_SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vertex.spv"));
const FRAGMENT_SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fragment.spv"));

#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    view_projection: [f32; 16],
    material: u32,
    outline: u32,
    outline_scale: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MaterialData {
    base: [f32; 4],
    shade: [f32; 3],
    blend: f32,
    shading_shift: f32,
    shading_toony: f32,
    cutoff: f32,
    equalization: f32,
    outline_color: [f32; 3],
    outline_lighting_mix: f32,
    outline_width: f32,
    outline_screen: f32,
    padding: [f32; 2],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PipelineKey {
    blend: bool,
    cull: Cull,
    depth_write: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Cull {
    None,
    Back,
    Front,
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
    material: u32,
    outline: bool,
}

type Buffer = (vk::Buffer, vk::DeviceMemory, *mut u8);

pub struct Renderer {
    gpu: Rc<Gpu>,
    pub eye: [u32; 2],
    targets: Vec<Target>,
    depth: Texture,
    render_pass: vk::RenderPass,
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipelines: HashMap<PipelineKey, vk::Pipeline>,
    sampler: vk::Sampler,
    outline_scale: f32,
    next: usize,
    last: Option<usize>,
}

pub struct Appearance {
    gpu: Rc<Gpu>,
    descriptors: vk::DescriptorPool,
    textures: Vec<Texture>,
    vertices: Buffer,
    indices: Buffer,
    palette: Buffer,
    joints: usize,
    materials: Buffer,
    drawn: Vec<DrawCall>,
}

fn color_range(levels: u32) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(levels).layer_count(1)
}

fn barrier(image: vk::Image, range: vk::ImageSubresourceRange, from: vk::ImageLayout, to: vk::ImageLayout, src: vk::AccessFlags, dst: vk::AccessFlags) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default().old_layout(from).new_layout(to).src_access_mask(src).dst_access_mask(dst).image(image).subresource_range(range)
}

impl Renderer {
    pub fn new(gpu: Rc<Gpu>, eye: [u32; 2], outline_scale: f32) -> Result<Renderer, String> {
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

        let both = vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT;
        let bindings = [
            vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(1).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default().binding(2).descriptor_type(vk::DescriptorType::SAMPLER).descriptor_count(1).stage_flags(both),
            vk::DescriptorSetLayoutBinding::default().binding(3).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::VERTEX),
            vk::DescriptorSetLayoutBinding::default().binding(4).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::VERTEX),
            vk::DescriptorSetLayoutBinding::default().binding(5).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1).stage_flags(both),
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
        let mut renderer = Renderer {
            depth: Texture { image: vk::Image::null(), memory: vk::DeviceMemory::null(), view: vk::ImageView::null() },
            gpu,
            eye,
            targets: Vec::new(),
            render_pass,
            set_layout,
            layout,
            pipelines: HashMap::new(),
            sampler,
            outline_scale,
            next: 0,
            last: None,
        };
        renderer.depth = renderer.attachment(DEPTH, vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT, vk::ImageAspectFlags::DEPTH)?;
        for _ in 0..RING {
            let target = renderer.target()?;
            renderer.targets.push(target);
        }
        Ok(renderer)
    }

    pub fn appearance(&mut self, model: &Model, skinned: &Skinned) -> Result<Appearance, String> {
        let key = |material: &crate::vrm::Material, outline: bool| PipelineKey {
            blend: material.alpha == Alpha::Blend,
            cull: if outline { Cull::Front } else if material.double_sided { Cull::None } else { Cull::Back },
            depth_write: material.depth_write,
        };
        let keys: Vec<PipelineKey> = model.materials.iter().flat_map(|material| [Some(key(material, false)), material.outline.as_ref().map(|_| key(material, true))]).flatten().filter(|key| !self.pipelines.contains_key(key)).collect();
        let created = self.pipelines(&keys)?;
        self.pipelines.extend(created);

        let gpu = &self.gpu;
        let device = &gpu.device;
        let set_count = model.materials.len() as u32;
        let sizes = [
            vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLED_IMAGE, descriptor_count: 3 * set_count },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLER, descriptor_count: set_count },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 2 * set_count },
        ];
        let descriptors = unsafe { device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(set_count).pool_sizes(&sizes), None) }.map_err(vk_error("vkCreateDescriptorPool"))?;
        let upload = |bytes: &[u8], usage| -> Result<Buffer, String> {
            let buffer = gpu.host_buffer(bytes.len() as u64, usage)?;
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.2, bytes.len()) };
            Ok(buffer)
        };
        let joints = skinned.joints();
        let material_data: Vec<MaterialData> = model.materials.iter().map(material_data).collect();
        let mut appearance = Appearance {
            gpu: gpu.clone(),
            descriptors,
            textures: Vec::new(),
            vertices: upload(bytes_of(&skinned.vertices), vk::BufferUsageFlags::VERTEX_BUFFER)?,
            indices: upload(bytes_of(&skinned.indices), vk::BufferUsageFlags::INDEX_BUFFER)?,
            palette: gpu.host_buffer((joints.max(1) * std::mem::size_of::<Mat4>()) as u64, vk::BufferUsageFlags::STORAGE_BUFFER)?,
            joints,
            materials: upload(bytes_of(&material_data), vk::BufferUsageFlags::STORAGE_BUFFER)?,
            drawn: Vec::new(),
        };
        for image in &model.images {
            appearance.textures.push(texture(gpu, image.width, image.height, &image.rgba)?);
        }
        let white = appearance.textures.len();
        appearance.textures.push(texture(gpu, 1, 1, &[255; 4])?);
        let layouts = vec![self.set_layout; model.materials.len()];
        let sets = unsafe { device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(descriptors).set_layouts(&layouts)) }.map_err(vk_error("vkAllocateDescriptorSets"))?;
        for (material, &set) in model.materials.iter().zip(&sets) {
            let image = |index: Option<usize>| [vk::DescriptorImageInfo::default().image_view(appearance.textures[index.unwrap_or(white)].view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let (base, shade, width) = (image(material.base_image), image(material.shade_image), image(material.outline.as_ref().and_then(|outline| outline.width_image)));
            let sampler = [vk::DescriptorImageInfo::default().sampler(self.sampler)];
            let palette = [vk::DescriptorBufferInfo::default().buffer(appearance.palette.0).range(vk::WHOLE_SIZE)];
            let materials = [vk::DescriptorBufferInfo::default().buffer(appearance.materials.0).range(vk::WHOLE_SIZE)];
            let writes = [
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&base),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&shade),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(2).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&sampler),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(3).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&width),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(4).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&palette),
                vk::WriteDescriptorSet::default().dst_set(set).dst_binding(5).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&materials),
            ];
            unsafe { device.update_descriptor_sets(&writes, &[]) };
        }
        appearance.drawn = skinned
            .draws
            .iter()
            .flat_map(|draw| {
                let material = &model.materials[draw.material];
                let call = |outline| DrawCall { first: draw.first, count: draw.count, set: sets[draw.material], pipeline: self.pipelines[&key(material, outline)], material: draw.material as u32, outline };
                [Some(call(false)), material.outline.as_ref().map(|_| call(true))]
            })
            .flatten()
            .collect();
        Ok(appearance)
    }

    fn pipelines(&self, keys: &[PipelineKey]) -> Result<HashMap<PipelineKey, vk::Pipeline>, String> {
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
            vk::VertexInputAttributeDescription::default().location(3).format(vk::Format::R16G16B16A16_UINT).offset(32),
            vk::VertexInputAttributeDescription::default().location(4).format(vk::Format::R32G32B32A32_SFLOAT).offset(40),
        ];
        let input = vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&vertex_bindings).vertex_attribute_descriptions(&vertex_attributes);
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let mut pipelines = HashMap::new();
        for &key in keys {
            if pipelines.contains_key(&key) {
                continue;
            }
            {
                let cull = match key.cull {
                    Cull::None => vk::CullModeFlags::NONE,
                    Cull::Back => vk::CullModeFlags::BACK,
                    Cull::Front => vk::CullModeFlags::FRONT,
                };
                let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(cull).front_face(vk::FrontFace::COUNTER_CLOCKWISE).line_width(1.0);
                let depth = vk::PipelineDepthStencilStateCreateInfo::default().depth_test_enable(true).depth_write_enable(key.depth_write).depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
                let attachment = [vk::PipelineColorBlendAttachmentState::default()
                    .blend_enable(key.blend)
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
                pipelines.insert(key, created[0]);
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

    pub fn render(&mut self, appearance: &Appearance, eyes: [Option<Mat4>; 2]) -> Result<openvr_sys::VRVulkanTextureData_t, String> {
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
            {
                device.cmd_bind_vertex_buffers(target.commands, 0, &[appearance.vertices.0], &[0]);
                device.cmd_bind_index_buffer(target.commands, appearance.indices.0, 0, vk::IndexType::UINT32);
                for (side, view_projection) in eyes.iter().enumerate() {
                    let Some(view_projection) = view_projection else { continue };
                    let x = (side as u32 * width) as f32;
                    device.cmd_set_viewport(target.commands, 0, &[vk::Viewport { x, y: 0.0, width: width as f32, height: height as f32, min_depth: 0.0, max_depth: 1.0 }]);
                    device.cmd_set_scissor(target.commands, 0, &[vk::Rect2D { offset: vk::Offset2D { x: x as i32, y: 0 }, extent: vk::Extent2D { width, height } }]);
                    for drawn in &appearance.drawn {
                        let push = Push { view_projection: view_projection.to_cols_array(), material: drawn.material, outline: drawn.outline as u32, outline_scale: self.outline_scale };
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

    pub fn read(&self) -> Result<Vec<u8>, String> {
        let [width, height] = self.eye;
        self.gpu.read(self.targets[self.last.ok_or("nothing rendered")?].texture.image, width * 2, height)
    }
}

fn bytes_of<T: Copy>(values: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, std::mem::size_of_val(values)) }
}

fn material_data(material: &crate::vrm::Material) -> MaterialData {
    let outline = material.outline.as_ref();
    MaterialData {
        base: material.base,
        shade: material.shade,
        blend: (material.alpha == Alpha::Blend) as u8 as f32,
        shading_shift: material.shading_shift,
        shading_toony: material.shading_toony,
        cutoff: match material.alpha {
            Alpha::Cutout(cutoff) => cutoff,
            _ => 0.0,
        },
        equalization: material.equalization,
        outline_color: outline.map_or([0.0; 3], |outline| outline.color),
        outline_lighting_mix: outline.map_or(0.0, |outline| outline.lighting_mix),
        outline_width: outline.map_or(0.0, |outline| outline.factor),
        outline_screen: outline.is_some_and(|outline| outline.width == OutlineWidth::Screen) as u8 as f32,
        padding: [0.0; 2],
    }
}

impl Appearance {
    pub fn update(&mut self, palette: &[Mat4], vertices: &[Vertex], changed: &[u32]) {
        assert_eq!(palette.len(), self.joints, "the palette has one matrix per joint");
        let bytes = bytes_of(palette);
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.palette.2, bytes.len()) };
        let target = self.vertices.2 as *mut Vertex;
        for &vertex in changed {
            unsafe { target.add(vertex as usize).write(vertices[vertex as usize]) };
        }
    }

}

fn destroy(device: &ash::Device, texture: &Texture) {
    unsafe {
        device.destroy_image_view(texture.view, None);
        device.destroy_image(texture.image, None);
        device.free_memory(texture.memory, None);
    }
}

impl Drop for Appearance {
    fn drop(&mut self) {
        let device = &self.gpu.device;
        unsafe {
            let _ = device.device_wait_idle();
            for buffer in [self.vertices, self.indices, self.palette, self.materials] {
                self.gpu.free_buffer(buffer);
            }
            for texture in &self.textures {
                destroy(device, texture);
            }
            device.destroy_descriptor_pool(self.descriptors, None);
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let device = &self.gpu.device;
        unsafe {
            let _ = device.device_wait_idle();
            for target in &self.targets {
                device.destroy_fence(target.fence, None);
                device.destroy_framebuffer(target.framebuffer, None);
                destroy(device, &target.texture);
            }
            destroy(device, &self.depth);
            for &pipeline in self.pipelines.values() {
                device.destroy_pipeline(pipeline, None);
            }
            device.destroy_sampler(self.sampler, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_render_pass(self.render_pass, None);
        }
    }
}

fn texture(gpu: &Gpu, width: u32, height: u32, rgba: &[u8]) -> Result<Texture, String> {
let device = &gpu.device;
    let levels = 32 - width.max(height).leading_zeros();
    let (image, memory) = gpu.image(
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
    let staging = gpu.host_buffer(rgba.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC)?;
    unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), staging.2, rgba.len()) };
    gpu.once(|commands| unsafe {
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
    gpu.free_buffer(staging);
    let view = unsafe { device.create_image_view(&vk::ImageViewCreateInfo::default().image(image).view_type(vk::ImageViewType::TYPE_2D).format(FORMAT).subresource_range(color_range(levels)), None) }.map_err(vk_error("vkCreateImageView"))?;
    Ok(Texture { image, memory, view })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrm::{tests::figure, Version};

    const COVERAGE: f32 = 0.95;
    const COLOR: f32 = 12.0;

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

    fn draw(model: &Model, pose: &crate::vrm::Pose, eye: [u32; 2], eyes: [Option<Mat4>; 2], shift: Vec3) -> Vec<u8> {
        draw_at(model, pose, eye, eyes, shift, crate::HEIGHT, crate::FLOOR)
    }

    fn draw_at(model: &Model, pose: &crate::vrm::Pose, eye: [u32; 2], eyes: [Option<Mat4>; 2], shift: Vec3, height: f32, floor: f32) -> Vec<u8> {
        let mut skinned = model.skinned().unwrap();
        let changed = skinned.morph(&pose.weights);
        let fit = crate::vrm::Fit::new(model, &skinned, height);
        let worlds = model.worlds(pose);
        let palette = skinned.palette(&worlds, Mat4::from_translation(shift) * fit.placement(model, &worlds, floor));
        let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), eye, height / PAGE_HEIGHT).unwrap();
        let mut appearance = renderer.appearance(model, &skinned).unwrap();
        appearance.update(&palette, &skinned.vertices, &changed);
        renderer.render(&appearance, eyes).unwrap();
        renderer.read().unwrap()
    }

    fn rendered(version: Version, eyes: [Vec3; 2]) -> (Vec<u8>, u32) {
        let model = Model::parse(&figure(version)).unwrap();
        (draw(&model, &model.rest(), [64, 64], eyes.map(|eye| eye_projection(eye, 0.2, 0.2)), Vec3::ZERO), 128)
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
        let pixels = draw(&model, &model.rest(), [64, 64], [Vec3::new(-0.032, 0.0, 0.4), Vec3::new(0.032, 0.0, 0.4)].map(|eye| eye_projection(eye, 0.2, 0.2)), Vec3::new(0.0, 0.0, 0.05));
        let [left, right] = [0, 1].map(|eye| covered_columns(&pixels, row, eye, 58));
        assert!(left[0] > right[0], "in front of the window the left eye sees it further right: {left:?} {right:?}");
    }

    #[test]
    fn a_transparent_surface_that_writes_depth_hides_a_later_one_behind_it() {
        let mut builder = crate::vrm::tests::Builder::new(serde_json::json!({ "VRMC_vrm": { "humanoid": { "humanBones": {} } } }));
        let quad = |z: f32| [-1.0, 0.0, z, 1.0, 0.0, z, 1.0, 1.0, z, -1.0, 1.0, z];
        let positions: Vec<f32> = quad(0.5).iter().chain(&quad(0.0)).copied().collect();
        let position = builder.accessor("VEC3", &positions);
        let normal = builder.accessor("VEC3", &[0.0, 0.0, 1.0].repeat(8));
        let near = builder.indices(&[0, 1, 2, 0, 2, 3]);
        let far = builder.indices(&[4, 5, 6, 4, 6, 7]);
        builder.set("nodes", serde_json::json!([{ "mesh": 0 }]));
        builder.set("meshes", serde_json::json!([{ "primitives": [
            { "attributes": { "POSITION": position, "NORMAL": normal }, "indices": near, "material": 0 },
            { "attributes": { "POSITION": position, "NORMAL": normal }, "indices": far, "material": 1 }
        ] }]));
        let toon = |zwrite: bool, color: [f32; 4]| serde_json::json!({ "alphaMode": "BLEND", "pbrMetallicRoughness": { "baseColorFactor": color }, "extensions": { "VRMC_materials_mtoon": { "transparentWithZWrite": zwrite } } });
        builder.set("materials", serde_json::json!([toon(true, [1.0, 0.0, 0.0, 0.5]), toon(false, [0.0, 0.0, 1.0, 0.5])]));
        let model = Model::parse(&builder.glb()).unwrap();
        let pixels = draw(&model, &model.rest(), [32, 32], [eye_projection(Vec3::new(0.0, 0.0, 0.4), 0.2, 0.2), None], Vec3::ZERO);
        let center = pixel(&pixels, 64, 16, 16);
        assert!(center[0] > 100 && center[2] == 0, "only the near surface shows: {center:?}");
    }

    #[test]
    fn a_new_appearance_draws_in_place_on_the_same_device() {
        let mut renderer = Renderer::new(Rc::new(Gpu::new(None).unwrap()), [32, 32], crate::HEIGHT / PAGE_HEIGHT).unwrap();
        let eyes = [eye_projection(Vec3::new(0.0, 0.0, 0.4), 0.2, 0.2), None];
        let mut centre = |model: &Model, appearance: &mut Option<Appearance>| {
            let skinned = model.skinned().unwrap();
            let fit = crate::vrm::Fit::new(model, &skinned, crate::HEIGHT);
            let worlds = model.worlds(&model.rest());
            let loaded = renderer.appearance(model, &skinned).unwrap();
            let shown = appearance.insert(loaded);
            shown.update(&skinned.palette(&worlds, fit.placement(model, &worlds, crate::FLOOR)), &skinned.vertices, &[]);
            renderer.render(shown, eyes).unwrap();
            pixel(&renderer.read().unwrap(), 64, 16, 16)
        };
        let mut shown = None;
        let red = centre(&crate::vrm::tests::plain([1.0, 0.0, 0.0, 1.0]), &mut shown);
        assert!(red[0] > 100 && red[2] == 0 && red[3] == 255, "{red:?}");
        let blue = centre(&crate::vrm::tests::plain([0.0, 0.0, 1.0, 1.0]), &mut shown);
        assert!(blue[2] > 100 && blue[0] == 0 && blue[3] == 255, "{blue:?}");
        let red = centre(&crate::vrm::tests::plain([1.0, 0.0, 0.0, 1.0]), &mut shown);
        assert!(red[0] > 100 && red[2] == 0, "and back: {red:?}");
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Golden {
        eye: [f32; 3],
        size: u32,
        time: f32,
        quad: f32,
        floor: f32,
        height: f32,
        page_height: f32,
    }

    fn golden() -> Golden {
        serde_json::from_str(include_str!("../golden/pose.json")).unwrap()
    }

    #[test]
    fn the_golden_pose_is_framed_as_the_host_frames_the_puppet() {
        let golden = golden();
        assert_eq!([golden.quad, golden.floor, golden.height, golden.page_height], [crate::QUAD, crate::FLOOR, crate::HEIGHT, PAGE_HEIGHT]);
    }

    fn golden_render(model: &Model, motion: &[u8]) -> Vec<u8> {
        let golden = golden();
        let standing = model.standing();
        let mut clip = crate::motion::Clip::parse(motion, model.version, model.rest_hips()).unwrap();
        clip.anchor(&standing);
        let mut animator = crate::motion::Animator::new([("idle".to_owned(), clip)].into(), crate::motion::Random::seeded());
        animator.update(golden.time);
        let mut pose = model.pose(&animator.humanoid(&standing, model.rest_hips()));
        model.express(&mut pose, "blink", 1.0);
        let eyes = [eye_projection(Vec3::from(golden.eye), golden.quad / 2.0, golden.quad / 2.0), None];
        let pixels = draw_at(model, &pose, [golden.size; 2], eyes, Vec3::ZERO, golden.height, golden.floor);
        let row = (golden.size * 8) as usize;
        pixels.chunks_exact(row).flat_map(|line| &line[..row / 2]).copied().collect()
    }

    struct Difference {
        coverage: f32,
        color: f32,
    }

    fn difference(native: &[u8], page: &[u8]) -> Difference {
        let (mut both, mut either, mut error) = (0usize, 0usize, 0f64);
        for (a, b) in native.chunks_exact(4).zip(page.chunks_exact(4)) {
            let (in_a, in_b) = (a[3] > 127, b[3] > 127);
            either += (in_a || in_b) as usize;
            both += (in_a && in_b) as usize;
            if in_a || in_b {
                error += (0..3).map(|channel| (a[channel] as f64 - b[channel] as f64).abs()).sum::<f64>() / 3.0;
            }
        }
        Difference { coverage: both as f32 / either.max(1) as f32, color: (error / either.max(1) as f64) as f32 }
    }

    #[test]
    fn the_native_render_of_the_golden_pose_matches_the_page() {
        let model = Model::parse(include_bytes!("../golden/figure.vrm")).unwrap();
        let native = golden_render(&model, include_bytes!("../golden/idle.json"));
        let mut page = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&include_bytes!("../golden/page.rgba.gz")[..]), &mut page).unwrap();
        let difference = difference(&native, &page);
        eprintln!("golden: silhouette overlap {:.4}, mean colour error {:.2}/255", difference.coverage, difference.color);
        assert!(difference.coverage >= COVERAGE, "the native silhouette overlaps the page's by {:.4}", difference.coverage);
        assert!(difference.color <= COLOR, "the native colours differ from the page's by {:.2}/255 on average", difference.color);
    }
}
