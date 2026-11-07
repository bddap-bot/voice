use openvr_sys as sys;
use std::ffi::{CStr, CString};

use crate::placement::{Hand, HandPose, Pose, Vec3};

/// The least rate the overlay draws at.
const OVERLAY_RATE: f32 = 72.0;
/// What share of a frame the overlay's drawing may take, the rest left to the compositor.
const DRAWING_SHARE: f32 = 0.75;

pub struct Runtime {
    system: &'static sys::VR_IVRSystem_FnTable,
    overlay: &'static sys::VR_IVROverlay_FnTable,
    compositor: &'static sys::VR_IVRCompositor_FnTable,
    render_models: &'static sys::VR_IVRRenderModels_FnTable,
    /// Render models loaded while others of the same device still load, kept so a slow one does not send the rest back to loading.
    loaded: std::cell::RefCell<std::collections::HashMap<CString, *mut sys::RenderModel_t>>,
}

fn table<T>(version: &[u8]) -> Result<&'static T, String> {
    let name = CString::new(format!("FnTable:{}", CStr::from_bytes_with_nul(version).unwrap().to_str().unwrap())).unwrap();
    let mut error = 0;
    let pointer = unsafe { sys::VR_GetGenericInterface(name.as_ptr(), &mut error) } as *const T;
    if error != 0 || pointer.is_null() {
        return Err(format!("{name:?}: {}", init_error(error)));
    }
    Ok(unsafe { &*pointer })
}

fn init_error(error: sys::EVRInitError) -> String {
    unsafe { CStr::from_ptr(sys::VR_GetVRInitErrorAsEnglishDescription(error)) }.to_string_lossy().into_owned()
}

macro_rules! call {
    ($table:expr, $name:ident $(, $arg:expr)*) => {
        unsafe { ($table.$name.expect(stringify!($name)))($($arg),*) }
    };
}

pub enum Signal {
    Quit,
}

impl Runtime {
    pub fn init() -> Result<Runtime, String> {
        let mut error = 0;
        unsafe { sys::VR_InitInternal(&mut error, sys::EVRApplicationType_VRApplication_Overlay) };
        if error != 0 {
            return Err(init_error(error));
        }
        Ok(Runtime { system: table(sys::IVRSystem_Version)?, overlay: table(sys::IVROverlay_Version)?, compositor: table(sys::IVRCompositor_Version)?, render_models: table(sys::IVRRenderModels_Version)?, loaded: Default::default() })
    }

    pub fn instance_extensions(&self) -> Vec<CString> {
        let mut buffer = vec![0 as std::os::raw::c_char; 4096];
        call!(self.compositor, GetVulkanInstanceExtensionsRequired, buffer.as_mut_ptr(), buffer.len() as u32);
        split(&buffer)
    }

    pub fn device_extensions(&self, physical: u64) -> Vec<CString> {
        let mut buffer = vec![0 as std::os::raw::c_char; 4096];
        call!(self.compositor, GetVulkanDeviceExtensionsRequired, physical as *mut sys::VkPhysicalDevice_T, buffer.as_mut_ptr(), buffer.len() as u32);
        split(&buffer)
    }

    pub fn output_device(&self, instance: u64) -> u64 {
        let mut device = 0u64;
        call!(self.system, GetOutputDevice, &mut device, sys::ETextureType_TextureType_Vulkan, instance as *mut sys::VkInstance_T);
        device
    }

    pub fn poll(&self) -> Option<Signal> {
        let mut event: sys::VREvent_t = unsafe { std::mem::zeroed() };
        while call!(self.system, PollNextEvent, &mut event, std::mem::size_of::<sys::VREvent_t>() as u32) {
            if event.eventType == sys::EVREventType_VREvent_Quit as u32 {
                call!(self.system, AcknowledgeQuit_Exiting);
                return Some(Signal::Quit);
            }
        }
        None
    }

    pub fn poses(&self) -> Vec<sys::TrackedDevicePose_t> {
        let mut poses: Vec<sys::TrackedDevicePose_t> = vec![unsafe { std::mem::zeroed() }; sys::k_unMaxTrackedDeviceCount as usize];
        call!(self.system, GetDeviceToAbsoluteTrackingPose, sys::ETrackingUniverseOrigin_TrackingUniverseStanding, 0.0, poses.as_mut_ptr(), poses.len() as u32);
        poses
    }

    pub fn head(&self, poses: &[sys::TrackedDevicePose_t]) -> Option<Pose> {
        let head = &poses[sys::k_unTrackedDeviceIndex_Hmd as usize];
        head.bPoseIsValid.then(|| Pose::from_m34(&head.mDeviceToAbsoluteTracking.m))
    }

    pub fn wait_frame(&self) {
        call!(self.overlay, WaitFrameSync, 100);
    }

    pub fn eye_offsets(&self) -> [Pose; 2] {
        [sys::EVREye_Eye_Left, sys::EVREye_Eye_Right].map(|eye| Pose::from_m34(&call!(self.system, GetEyeToHeadTransform, eye).m))
    }

    /// The tangent of the widest either eye sees off the head's forward axis, across or up and down.
    pub fn view(&self) -> f32 {
        let offsets = self.eye_offsets();
        [sys::EVREye_Eye_Left, sys::EVREye_Eye_Right].into_iter().zip(offsets).fold(0.0, |widest, (eye, offset)| {
            let (mut left, mut right, mut top, mut bottom) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            call!(self.system, GetProjectionRaw, eye, &mut left, &mut right, &mut top, &mut bottom);
            [[left, top], [left, bottom], [right, top], [right, bottom]].into_iter().fold(widest, |widest, [x, y]| {
                let [x, y, z] = offset.rotate([x, y, -1.0]);
                widest.max((x / z).abs()).max((y / z).abs())
            })
        })
    }

    /// The display's refresh rate, None where SteamVR does not say.
    fn display_rate(&self) -> Option<f32> {
        let mut error = 0;
        let rate = call!(self.system, GetFloatTrackedDeviceProperty, sys::k_unTrackedDeviceIndex_Hmd as u32, sys::ETrackedDeviceProperty_Prop_DisplayFrequency_Float, &mut error);
        (error == 0 && rate > 0.0).then_some(rate)
    }

    /// How many refreshes each overlay frame spans: the compositor shows the last image at every refresh, so a display
    /// refreshing at a multiple of the overlay's rate gets a frame every that many.
    pub fn refreshes_per_frame(&self) -> u32 {
        self.display_rate().map_or(1, |rate| ((rate / OVERLAY_RATE).floor() as u32).max(1))
    }

    /// How long the overlay may take to draw a frame: a share of the refreshes it spans.
    pub fn drawing_budget(&self) -> Option<std::time::Duration> {
        self.display_rate().map(|rate| std::time::Duration::from_secs_f32(DRAWING_SHARE * self.refreshes_per_frame() as f32 / rate))
    }

    /// Waits for the refresh the next frame is drawn for: the first at least all but half a refresh of its span after
    /// the frame that `started`.
    pub fn wait_frames(&self, started: std::time::Instant) {
        let span = self.refreshes_per_frame();
        let least = self.display_rate().map_or(std::time::Duration::ZERO, |rate| std::time::Duration::from_secs_f32((span as f32 - 0.5) / rate));
        self.wait_frame();
        while started.elapsed() < least {
            self.wait_frame();
        }
    }

    /// The left eye's recommended render width over its horizontal field, in pixels per radian.
    pub fn display_density(&self) -> f32 {
        let (mut width, mut height) = (0, 0);
        call!(self.system, GetRecommendedRenderTargetSize, &mut width, &mut height);
        let (mut left, mut right, mut top, mut bottom) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        call!(self.system, GetProjectionRaw, sys::EVREye_Eye_Left, &mut left, &mut right, &mut top, &mut bottom);
        width as f32 / (right.atan() - left.atan())
    }

    pub fn hand_index(&self, hand: Hand) -> Option<u32> {
        let role = match hand {
            Hand::Left => sys::ETrackedControllerRole_TrackedControllerRole_LeftHand,
            Hand::Right => sys::ETrackedControllerRole_TrackedControllerRole_RightHand,
        };
        let index = call!(self.system, GetTrackedDeviceIndexForControllerRole, role);
        (index < sys::k_unMaxTrackedDeviceCount as u32).then_some(index)
    }

    fn render_model(&self, device: u32) -> Result<CString, String> {
        let mut buffer = vec![0 as std::os::raw::c_char; 256];
        let mut error = 0;
        call!(self.system, GetStringTrackedDeviceProperty, device, sys::ETrackedDeviceProperty_Prop_RenderModelName_String, buffer.as_mut_ptr(), buffer.len() as u32, &mut error);
        if error != 0 {
            return Err(format!("device {device} has no render model name (property error {error})"));
        }
        Ok(unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_owned())
    }

    /// The vertices of the device's render model, as drawn, in its tracking frame: its components' meshes where it has them, else its own; None while SteamVR still loads one.
    pub fn outline(&self, device: u32) -> Result<Option<Vec<Vec3>>, String> {
        let model = self.render_model(device)?;
        let count = call!(self.render_models, GetComponentCount, model.as_ptr() as *mut _);
        let mut meshes = Vec::new();
        for index in 0..count {
            let mut name = vec![0 as std::os::raw::c_char; 128];
            call!(self.render_models, GetComponentName, model.as_ptr() as *mut _, index, name.as_mut_ptr(), name.len() as u32);
            let mut mesh = vec![0 as std::os::raw::c_char; 256];
            if call!(self.render_models, GetComponentRenderModelName, model.as_ptr() as *mut _, name.as_mut_ptr(), mesh.as_mut_ptr(), mesh.len() as u32) == 0 {
                continue;
            }
            let mut controller: sys::VRControllerState_t = unsafe { std::mem::zeroed() };
            let mut mode: sys::RenderModel_ControllerMode_State_t = unsafe { std::mem::zeroed() };
            let mut state: sys::RenderModel_ComponentState_t = unsafe { std::mem::zeroed() };
            if !call!(self.render_models, GetComponentState, model.as_ptr() as *mut _, name.as_mut_ptr(), &mut controller, &mut mode, &mut state) || state.uProperties & sys::EVRComponentProperty_VRComponentProperty_IsVisible == 0 {
                continue;
            }
            meshes.push((unsafe { CStr::from_ptr(mesh.as_ptr()) }.to_owned(), Some(Pose::from_m34(&state.mTrackingToComponentRenderModel.m))));
        }
        if meshes.is_empty() {
            meshes.push((model.clone(), None));
        }
        let mut kept = self.loaded.borrow_mut();
        let mut waiting = false;
        let mut failed = None;
        for (mesh, _) in &meshes {
            if kept.contains_key(mesh) {
                continue;
            }
            let mut model_data: *mut sys::RenderModel_t = std::ptr::null_mut();
            match call!(self.render_models, LoadRenderModel_Async, mesh.as_ptr() as *mut _, &mut model_data) {
                sys::EVRRenderModelError_VRRenderModelError_None if !model_data.is_null() => {
                    kept.insert(mesh.clone(), model_data);
                }
                sys::EVRRenderModelError_VRRenderModelError_None => {}
                sys::EVRRenderModelError_VRRenderModelError_Loading => waiting = true,
                error => failed = failed.or(Some(format!("render model {mesh:?} of {model:?}: error {error}"))),
            }
        }
        if waiting && failed.is_none() {
            return Ok(None);
        }
        let mut points = Vec::new();
        for (mesh, placed) in &meshes {
            let Some(model_data) = kept.remove(mesh) else { continue };
            let (data, count) = unsafe { ((*model_data).rVertexData, (*model_data).unVertexCount as usize) };
            if !data.is_null() && failed.is_none() {
                let vertices = unsafe { std::slice::from_raw_parts(data, count) };
                points.extend(vertices.iter().map(|vertex| placed.map_or(vertex.vPosition.v, |placed| placed.apply(vertex.vPosition.v))));
            }
            call!(self.render_models, FreeRenderModel, model_data);
        }
        if let Some(error) = failed {
            return Err(error);
        }
        Ok(Some(points))
    }

    pub fn tip(&self, device: u32) -> Result<Vec3, String> {
        let model = self.render_model(device)?;
        let tip = CString::new("tip").unwrap();
        let mut controller: sys::VRControllerState_t = unsafe { std::mem::zeroed() };
        let mut mode: sys::RenderModel_ControllerMode_State_t = unsafe { std::mem::zeroed() };
        let mut state: sys::RenderModel_ComponentState_t = unsafe { std::mem::zeroed() };
        if call!(self.render_models, GetComponentState, model.as_ptr() as *mut _, tip.as_ptr() as *mut _, &mut controller, &mut mode, &mut state) {
            return Ok(Pose::from_m34(&state.mTrackingToComponentLocal.m).t);
        }
        let components: Vec<String> = (0..call!(self.render_models, GetComponentCount, model.as_ptr() as *mut _))
            .map(|index| {
                let mut name = vec![0 as std::os::raw::c_char; 128];
                call!(self.render_models, GetComponentName, model.as_ptr() as *mut _, index, name.as_mut_ptr(), name.len() as u32);
                unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy().into_owned()
            })
            .collect();
        Err(format!("render model {model:?} has no tip component; it has [{}]", components.join(", ")))
    }

    pub fn pulse(&self, device: u32) {
        call!(self.system, TriggerHapticPulse, device, 0, 3000);
    }

    pub fn hands(&self, poses: &[sys::TrackedDevicePose_t]) -> Vec<HandPose> {
        [Hand::Left, Hand::Right]
            .into_iter()
            .filter_map(|hand| {
                let pose = &poses[self.hand_index(hand)? as usize];
                pose.bPoseIsValid.then(|| HandPose { hand, pose: Pose::from_m34(&pose.mDeviceToAbsoluteTracking.m) })
            })
            .collect()
    }

    pub fn create_overlay(&self, key: &str, name: &str, width: f32, stereo: bool) -> Result<Overlay<'_>, String> {
        let key = CString::new(key).unwrap();
        let name = CString::new(name).unwrap();
        let mut handle = 0;
        let error = call!(self.overlay, CreateOverlay, key.as_ptr() as *mut _, name.as_ptr() as *mut _, &mut handle);
        if error != 0 {
            return Err(format!("CreateOverlay failed with {error}"));
        }
        call!(self.overlay, SetOverlayFlag, handle, sys::VROverlayFlags_SideBySide_Parallel, stereo);
        call!(self.overlay, SetOverlayFlag, handle, sys::VROverlayFlags_IsPremultiplied, true);
        call!(self.overlay, SetOverlayWidthInMeters, handle, width);
        Ok(Overlay { runtime: self, handle, shown: false, width, part: 1.0 })
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe { sys::VR_ShutdownInternal() };
        tracing::info!("SteamVR session closed");
    }
}

fn split(buffer: &[std::os::raw::c_char]) -> Vec<CString> {
    unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().split_whitespace().map(|name| CString::new(name).unwrap()).collect()
}

#[derive(Clone, Copy, PartialEq)]
pub enum Place {
    World(Pose),
    On(u32, Pose),
}

/// Where a rendered image is shown: a SteamVR overlay in the host, a fake in tests.
pub trait Surface {
    fn texture(&self, texture: &mut sys::VRVulkanTextureData_t) -> Result<(), String>;
    fn place(&self, place: &Place);
    fn resize(&mut self, _: f32) {}
    fn part(&mut self, _: f32) {}
    fn show(&mut self);
    fn hide(&mut self);

    /// SteamVR moves an overlay's quad the moment it is set but swaps its image only once the image is given,
    /// so geometry set first spends the frames in between showing the last image on the new quad.
    fn present(&mut self, texture: &mut sys::VRVulkanTextureData_t, place: &Place, width: f32, part: f32) -> Result<(), String> {
        self.texture(texture)?;
        self.place(place);
        self.resize(width);
        self.part(part);
        self.show();
        Ok(())
    }
}

/// Two surfaces taking turns to show one figure. SteamVR may show the previous image on a quad just moved, which
/// displaces the whole figure for a frame, so a quad that moves once and then holds is set on the hidden surface first
/// and shown a frame later, once the image drawn for it is in. A quad moving every frame moves in place. The texture
/// bounds change in place: a held window's texels follow the eye's distance and the drawing budget, and a frame's
/// late image only scales the figure by that step.
pub struct Handoff<S> {
    surfaces: [S; 2],
    quads: [Option<(Place, f32)>; 2],
    phase: Phase,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Holding,
    Staged,
    Moving,
}

impl<S: Surface> Handoff<S> {
    pub fn new(surfaces: [S; 2]) -> Handoff<S> {
        Handoff { surfaces, quads: [None; 2], phase: Phase::Holding }
    }

    pub fn present(&mut self, texture: &mut sys::VRVulkanTextureData_t, place: &Place, width: f32, part: f32) -> Result<(), String> {
        let quad = Some((*place, width));
        self.phase = match self.phase {
            _ if self.quads[0] == quad => Phase::Holding,
            Phase::Staged if self.quads[1] == quad => {
                self.surfaces[1].present(texture, place, width, part)?;
                self.surfaces[0].hide();
                self.surfaces.swap(0, 1);
                self.quads.swap(0, 1);
                self.phase = Phase::Holding;
                return Ok(());
            }
            Phase::Holding => {
                let back = &mut self.surfaces[1];
                back.texture(texture)?;
                back.place(place);
                back.resize(width);
                back.part(part);
                self.quads[1] = quad;
                self.phase = Phase::Staged;
                return Ok(());
            }
            Phase::Staged | Phase::Moving => Phase::Moving,
        };
        self.quads[0] = quad;
        self.surfaces[0].present(texture, place, width, part)
    }

    pub fn hide(&mut self) {
        self.phase = Phase::Holding;
        self.quads = [None; 2];
        self.surfaces.iter_mut().for_each(Surface::hide);
    }
}

impl Surface for Overlay<'_> {
    fn texture(&self, texture: &mut sys::VRVulkanTextureData_t) -> Result<(), String> {
        Overlay::texture(self, texture)
    }
    fn place(&self, place: &Place) {
        match place {
            Place::World(pose) => self.place_in_world(pose),
            Place::On(device, offset) => self.place_on(*device, offset),
        }
    }
    fn resize(&mut self, width: f32) {
        Overlay::resize(self, width)
    }
    fn part(&mut self, part: f32) {
        Overlay::part(self, part)
    }
    fn show(&mut self) {
        Overlay::show(self)
    }
    fn hide(&mut self) {
        Overlay::hide(self)
    }
}

pub struct Overlay<'a> {
    runtime: &'a Runtime,
    handle: sys::VROverlayHandle_t,
    shown: bool,
    width: f32,
    part: f32,
}

impl Overlay<'_> {
    pub fn place_on(&self, device: u32, offset: &Pose) {
        let mut matrix = sys::HmdMatrix34_t { m: offset.to_m34() };
        call!(self.runtime.overlay, SetOverlayTransformTrackedDeviceRelative, self.handle, device, &mut matrix);
    }

    pub fn place_in_world(&self, pose: &Pose) {
        let mut matrix = sys::HmdMatrix34_t { m: pose.to_m34() };
        call!(self.runtime.overlay, SetOverlayTransformAbsolute, self.handle, sys::ETrackingUniverseOrigin_TrackingUniverseStanding, &mut matrix);
    }

    fn resize(&mut self, width: f32) {
        if width != self.width {
            call!(self.runtime.overlay, SetOverlayWidthInMeters, self.handle, width);
            self.width = width;
        }
    }

    /// Shows the top left `part` of each eye's texture across the whole overlay, the eyes packed side by side as `Renderer::render` packs them.
    fn part(&mut self, part: f32) {
        if part != self.part {
            let mut bounds = sys::VRTextureBounds_t { uMin: 0.0, vMin: 0.0, uMax: part, vMax: part };
            call!(self.runtime.overlay, SetOverlayTextureBounds, self.handle, &mut bounds);
            self.part = part;
        }
    }

    pub fn above_others(&self) {
        call!(self.runtime.overlay, SetOverlaySortOrder, self.handle, 1);
    }

    pub fn texture(&self, texture: &mut sys::VRVulkanTextureData_t) -> Result<(), String> {
        let mut texture = sys::Texture_t { handle: texture as *mut _ as *mut std::ffi::c_void, eType: sys::ETextureType_TextureType_Vulkan, eColorSpace: sys::EColorSpace_ColorSpace_Auto };
        let error = call!(self.runtime.overlay, SetOverlayTexture, self.handle, &mut texture);
        if error != 0 {
            return Err(format!("SetOverlayTexture failed with {error}"));
        }
        Ok(())
    }

    pub fn show(&mut self) {
        if !self.shown {
            call!(self.runtime.overlay, ShowOverlay, self.handle);
            self.shown = true;
        }
    }

    pub fn hide(&mut self) {
        if self.shown {
            call!(self.runtime.overlay, HideOverlay, self.handle);
            self.shown = false;
        }
    }
}

impl Drop for Overlay<'_> {
    fn drop(&mut self) {
        call!(self.runtime.overlay, DestroyOverlay, self.handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorded(std::cell::RefCell<Vec<&'static str>>);

    impl Surface for Recorded {
        fn texture(&self, _: &mut sys::VRVulkanTextureData_t) -> Result<(), String> {
            self.0.borrow_mut().push("texture");
            Ok(())
        }
        fn place(&self, _: &Place) {
            self.0.borrow_mut().push("place");
        }
        fn resize(&mut self, _: f32) {
            self.0.borrow_mut().push("resize");
        }
        fn part(&mut self, _: f32) {
            self.0.borrow_mut().push("part");
        }
        fn show(&mut self) {
            self.0.borrow_mut().push("show");
        }
        fn hide(&mut self) {}
    }

    #[test]
    fn a_frame_gives_its_image_before_the_quad_it_belongs_to() {
        let mut surface = Recorded::default();
        let mut texture: sys::VRVulkanTextureData_t = unsafe { std::mem::zeroed() };
        surface.present(&mut texture, &Place::World(Pose { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] }), 1.0, 0.5).unwrap();
        assert_eq!(surface.0.into_inner(), ["texture", "place", "resize", "part", "show"]);
    }

    /// An overlay as the compositor sees it: the quad as last set, and each image given with the frame it was drawn in.
    #[derive(Default)]
    struct Composited {
        place: Option<Place>,
        given: Vec<u64>,
        shown: bool,
    }

    struct Shared(std::rc::Rc<std::cell::RefCell<Composited>>);

    impl Surface for Shared {
        fn texture(&self, texture: &mut sys::VRVulkanTextureData_t) -> Result<(), String> {
            self.0.borrow_mut().given.push(texture.m_nImage);
            Ok(())
        }
        fn place(&self, place: &Place) {
            self.0.borrow_mut().place = Some(*place);
        }
        fn show(&mut self) {
            self.0.borrow_mut().shown = true;
        }
        fn hide(&mut self) {
            self.0.borrow_mut().shown = false;
        }
    }

    fn at(x: f32) -> Place {
        Place::World(Pose { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [x, 1.0, -1.0] })
    }

    /// Presents a window at each of `quads` in turn, one frame each (`None` hides it), its texture bounds changing every
    /// frame as a held window's texels do, and gives what the compositor could show on each frame when it latches images
    /// `lag` frames late: per shown surface, the frame its image was drawn in and the quad it is on.
    fn composite(quads: &[Option<f32>], lag: usize) -> Vec<Vec<(u64, Place)>> {
        let surfaces = [(); 2].map(|_| std::rc::Rc::new(std::cell::RefCell::new(Composited::default())));
        let mut handoff = Handoff::new(surfaces.clone().map(Shared));
        quads.iter().enumerate().map(|(frame, &x)| {
            let mut texture: sys::VRVulkanTextureData_t = unsafe { std::mem::zeroed() };
            texture.m_nImage = frame as u64;
            match x {
                Some(x) => handoff.present(&mut texture, &at(x), 1.0, 1.0 - frame as f32 * 1e-3).unwrap(),
                None => handoff.hide(),
            }
            surfaces.iter().map(|surface| surface.borrow()).filter(|surface| surface.shown).filter_map(|surface| {
                let latched = surface.given.iter().rev().find(|&&given| given as usize + lag <= frame).or(surface.given.first())?;
                Some((*latched, surface.place?))
            }).collect()
        }).collect()
    }

    /// Asserts that from `from` on, every shown image is on the quad it was drawn for and at most one frame older than
    /// the latch allows, and that the figure is shown on every frame but the one after a hidden figure reappears.
    fn paired(quads: &[Option<f32>], from: usize) {
        for lag in [0, 1] {
            for (frame, shown) in composite(quads, lag).iter().enumerate().skip(from) {
                if quads[frame].is_none() {
                    assert!(shown.is_empty(), "lag {lag}, frame {frame}: shown while hidden");
                    continue;
                }
                assert!(!shown.is_empty() || quads[frame - 1].is_none(), "lag {lag}, frame {frame}: nothing shown");
                for &(drawn, place) in shown {
                    assert!(quads[drawn as usize].map(at) == Some(place), "lag {lag}, frame {frame}: the image drawn in frame {drawn} for {:?} is on another quad", quads[drawn as usize]);
                    assert!(drawn as usize + 1 + lag >= frame, "lag {lag}, frame {frame}: the figure froze on frame {drawn}");
                }
            }
        }
    }

    #[test]
    fn a_refitted_window_never_shows_an_image_drawn_for_another_quad() {
        let quads: Vec<Option<f32>> = [0.0, 0.3, -0.2, 0.25].iter().flat_map(|&x| [Some(x); 6]).chain([None, None]).chain([Some(0.25); 6]).chain([None]).chain([Some(0.4); 6]).collect();
        paired(&quads, 1);
    }

    #[test]
    fn a_window_moving_every_frame_keeps_showing_the_latest_image_and_hands_off_once_it_holds() {
        let quads: Vec<Option<f32>> = (0..20).map(|frame| Some(frame as f32 * 0.01)).chain([None]).chain([Some(0.5); 6]).collect();
        for (frame, shown) in composite(&quads, 0).iter().enumerate().take(20).skip(2) {
            assert_eq!(shown.iter().map(|&(drawn, _)| drawn).collect::<Vec<_>>(), [frame as u64], "frame {frame}");
        }
        paired(&quads, 20);
    }
}
