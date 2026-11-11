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

    pub fn poses(&self, ahead: std::time::Duration) -> Vec<sys::TrackedDevicePose_t> {
        let mut poses: Vec<sys::TrackedDevicePose_t> = vec![unsafe { std::mem::zeroed() }; sys::k_unMaxTrackedDeviceCount as usize];
        call!(self.system, GetDeviceToAbsoluteTrackingPose, sys::ETrackingUniverseOrigin_TrackingUniverseStanding, ahead.as_secs_f32(), poses.as_mut_ptr(), poses.len() as u32);
        poses
    }

    /// How long until a frame whose image is ready `ready` from now is seen. A frame drawn for the eyes as they are
    /// now is seen that much later, and whatever of the figure lies off the window's plane swims against the window
    /// while the head moves.
    pub fn seen_in(&self, ready: std::time::Duration) -> std::time::Duration {
        let (mut since, mut counter) = (0.0f32, 0u64);
        let (Some(rate), true) = (self.display_rate(), call!(self.system, GetTimeSinceLastVsync, &mut since, &mut counter)) else { return std::time::Duration::ZERO };
        seen_after(since, ready.as_secs_f32(), rate, self.refreshes_per_frame(), self.headset_float(sys::ETrackedDeviceProperty_Prop_SecondsFromVsyncToPhotons_Float).unwrap_or(0.0))
    }

    fn headset_float(&self, property: sys::ETrackedDeviceProperty) -> Option<f32> {
        let mut error = 0;
        let value = call!(self.system, GetFloatTrackedDeviceProperty, sys::k_unTrackedDeviceIndex_Hmd as u32, property, &mut error);
        (error == 0 && value.is_finite() && value >= 0.0).then_some(value)
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
        self.headset_float(sys::ETrackedDeviceProperty_Prop_DisplayFrequency_Float).filter(|&rate| rate > 0.0)
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

    pub fn handoff(&self, key: &str, name: &str, width: f32) -> Result<Handoff<Overlay<'_>>, String> {
        Ok(Handoff::new([self.create_overlay(key, name, width, true)?, self.create_overlay(&format!("{key}.handoff"), name, width, true)?]))
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

/// The compositor composes each refresh from the images ready at its vsync and lights it a refresh plus `photons`
/// later, and a frame holds for `span` refreshes: the time from now, `since` the last vsync, to the middle of them.
/// A stalled frame's `ready` is held to two spans, so one stall does not throw every pose far ahead.
fn seen_after(since: f32, ready: f32, rate: f32, span: u32, photons: f32) -> std::time::Duration {
    let refresh = 1.0 / rate;
    let held = span.max(1) as f32;
    let taken = ((since + ready.min(2.0 * held * refresh)) / refresh).ceil() * refresh;
    std::time::Duration::try_from_secs_f32(taken - since + refresh + photons + (held - 1.0) * refresh / 2.0).unwrap_or_default()
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

/// Two surfaces taking turns to show one figure. SteamVR applies a quad's place, width and texture bounds the moment
/// they are set but shows the image given with them only once it is drawn, so a shown surface would show the previous
/// image through them until then. A quad that changes is therefore set on the hidden surface with the image drawn
/// for it, and that surface is shown a frame later, when its image is in. Only a place that changes on every frame
/// moves in place, the image a frame behind it: handed over, it would trail the controller it rides.
pub struct Handoff<S> {
    surfaces: [S; 2],
    shown: Option<Geometry>,
    staged: Option<Geometry>,
    /// Whether the last frame's place differed from the one before it.
    moved: bool,
}

#[derive(Clone, Copy)]
struct Geometry {
    place: Place,
    width: f32,
    part: f32,
}

impl<S: Surface> Handoff<S> {
    pub fn new(surfaces: [S; 2]) -> Handoff<S> {
        Handoff { surfaces, shown: None, staged: None, moved: false }
    }

    pub fn present(&mut self, texture: &mut sys::VRVulkanTextureData_t, place: &Place, width: f32, part: f32) -> Result<(), String> {
        let quad = Geometry { place: *place, width, part };
        let moved = self.staged.or(self.shown).is_some_and(|last| last.place != *place);
        let moving = std::mem::replace(&mut self.moved, moved) && moved;
        if let Some(staged) = self.staged.take() {
            self.surfaces[1].show();
            self.surfaces[0].hide();
            self.surfaces.swap(0, 1);
            self.shown = Some(staged);
        }
        match self.shown {
            Some(shown) if (shown.width, shown.part) == (width, part) && (shown.place == *place || moving) => {
                self.surfaces[0].present(texture, place, width, part)?;
                self.shown = Some(quad);
            }
            _ => {
                let hidden = &mut self.surfaces[1];
                hidden.texture(texture)?;
                hidden.place(place);
                hidden.resize(width);
                hidden.part(part);
                self.staged = Some(quad);
            }
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        (self.shown, self.staged, self.moved) = (None, None, false);
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

    #[test]
    fn a_frame_is_drawn_for_the_eyes_when_it_is_seen() {
        let refresh = 1.0 / 144.0;
        let seen = |since: f32, ready: f32| seen_after(since, ready, 144.0, 2, 0.00976).as_secs_f32();
        assert!((seen(0.0019, 0.0168) - 0.0392).abs() < 0.0005, "the headset's measured mean time from pose to light, 39.2 ms");
        assert!((seen(0.0019, 0.004) - seen(0.0019, 0.0168) + 2.0 * refresh).abs() < 1e-5, "an image ready two refreshes sooner is seen two refreshes sooner");
        assert!((seen(0.0, 0.0) - (refresh + 0.00976 + refresh / 2.0)).abs() < 1e-5, "an image ready at a vsync is lit a refresh and the photon delay later, held for two");
        assert!((seen_after(0.0, 0.0, 144.0, 1, 0.0).as_secs_f32() - refresh).abs() < 1e-5, "a frame held for one refresh is seen at that one");
        assert_eq!(seen(0.0019, 0.5), seen(0.0019, 4.0 * refresh), "a stalled frame predicts no further than two spans of drawing");
    }

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

    /// An overlay as the compositor sees it: the quad, width and texture bounds as last set, and each image given
    /// with the frame it was drawn in.
    #[derive(Default)]
    struct Composited {
        place: Option<Place>,
        extent: (f32, f32),
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
        fn resize(&mut self, width: f32) {
            self.0.borrow_mut().extent.0 = width;
        }
        fn part(&mut self, part: f32) {
            self.0.borrow_mut().extent.1 = part;
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

    /// What a frame asks to show: where its quad stands, and the quad's width and the part of the texture its image
    /// fills. `None` hides the figure.
    type Asked = Option<(f32, (f32, f32))>;

    const WHOLE: (f32, f32) = (1.0, 1.0);

    /// Presents each of `frames` in turn and gives what the compositor could show on each when it latches images
    /// `lag` frames late: per shown surface, the frame its image was drawn in, the quad it is on and the width and
    /// texture bounds it is seen through.
    fn composite(frames: &[Asked], lag: usize) -> Vec<Vec<(u64, Place, (f32, f32))>> {
        let surfaces = [(); 2].map(|_| std::rc::Rc::new(std::cell::RefCell::new(Composited::default())));
        let mut handoff = Handoff::new(surfaces.clone().map(Shared));
        frames.iter().enumerate().map(|(frame, &asked)| {
            let mut texture: sys::VRVulkanTextureData_t = unsafe { std::mem::zeroed() };
            texture.m_nImage = frame as u64;
            match asked {
                Some((x, (width, part))) => handoff.present(&mut texture, &at(x), width, part).unwrap(),
                None => handoff.hide(),
            }
            surfaces.iter().map(|surface| surface.borrow()).filter(|surface| surface.shown).filter_map(|surface| {
                let latched = surface.given.iter().rev().find(|&&given| given as usize + lag <= frame).or(surface.given.first())?;
                Some((*latched, surface.place?, surface.extent))
            }).collect()
        }).collect()
    }

    /// Asserts that from `from` on, exactly one surface shows the figure on every frame but the first after it was
    /// hidden, its image seen through the width and texture bounds it was drawn for and at most one frame older
    /// than the latch allows. The image is on the quad it was drawn for too, or where the place is `moving` on every
    /// frame, on that quad or the next frame's.
    fn paired(frames: &[Asked], from: usize, moving: bool) {
        for lag in [0, 1] {
            for (frame, shown) in composite(frames, lag).iter().enumerate().skip(from) {
                if frames[frame].is_none() {
                    assert!(shown.is_empty(), "lag {lag}, frame {frame}: shown while hidden");
                    continue;
                }
                assert!(shown.len() == 1 || (shown.is_empty() && frames[frame - 1].is_none()), "lag {lag}, frame {frame}: {} surfaces shown", shown.len());
                for &(drawn, place, extent) in shown {
                    let drawn = drawn as usize;
                    let (x, asked) = frames[drawn].expect("an image is drawn only for a shown figure");
                    assert!(asked == extent, "lag {lag}, frame {frame}: the image drawn in frame {drawn} for width and bounds {asked:?} is seen through {extent:?}");
                    let next = frames.get(drawn + 1).copied().flatten().map(|(x, _)| at(x));
                    assert!(at(x) == place || (moving && next == Some(place)), "lag {lag}, frame {frame}: the image drawn in frame {drawn} for {x} is on another quad");
                    assert!(drawn + 1 + lag >= frame, "lag {lag}, frame {frame}: the figure froze on frame {drawn}");
                }
            }
        }
    }

    #[test]
    fn a_refitted_window_never_shows_an_image_drawn_for_another_quad() {
        let frames: Vec<Asked> = [0.0, 0.3, -0.2, 0.25].iter().flat_map(|&x| [Some((x, WHOLE)); 6]).chain([None, None]).chain([Some((0.25, WHOLE)); 6]).chain([None]).chain([Some((0.4, WHOLE)); 6]).collect();
        paired(&frames, 1, false);
        let moved_as_it_appears = [Some((0.0, WHOLE)), Some((0.3, WHOLE)), Some((0.3, WHOLE)), None, Some((0.1, WHOLE)), Some((0.2, WHOLE)), Some((0.2, WHOLE))];
        paired(&moved_as_it_appears, 1, false);
        let moved_again_two_frames_on: Vec<Asked> = [Some((0.0, WHOLE)); 4].into_iter().chain([Some((0.3, WHOLE)); 2]).chain([Some((0.6, WHOLE)); 4]).collect();
        paired(&moved_again_two_frames_on, 1, false);
        let hidden_as_it_moved: Vec<Asked> = [Some((0.0, WHOLE)); 3].into_iter().chain([Some((0.3, WHOLE)), None, None]).chain([Some((0.3, WHOLE)); 4]).collect();
        paired(&hidden_as_it_moved, 1, false);
    }

    #[test]
    fn a_held_window_never_shows_an_image_through_a_width_or_texture_bounds_it_was_not_drawn_for() {
        let bounds = [1.0, 0.9, 0.75, 0.9, 1.0].iter().flat_map(|&part| [Some((0.0, (1.0, part))); 5]);
        let widths = [1.2, 0.8].iter().flat_map(|&width| [Some((0.0, (width, 1.0))); 5]);
        let after_hiding = [None].into_iter().chain([Some((0.0, (1.0, 0.6))); 4]).chain([Some((0.3, (1.5, 0.5))); 4]);
        let then_a_move = [Some((0.3, (1.5, 0.4))), Some((0.6, (1.5, 0.4))), Some((0.6, (1.5, 0.4)))];
        paired(&bounds.chain(widths).chain(after_hiding).chain(then_a_move).collect::<Vec<_>>(), 2, false);
    }

    #[test]
    fn a_window_resized_on_every_frame_shows_each_image_through_its_own_width_and_bounds() {
        let growing: Vec<Asked> = (0..30).map(|frame| Some((0.0, (1.0 + frame as f32 * 0.05, 1.0 - frame as f32 * 0.01)))).chain([Some((0.0, (3.0, 0.5))); 4]).collect();
        paired(&growing, 2, false);
        let after_hiding: Vec<Asked> = [Some((0.0, WHOLE)); 4].into_iter().chain([None, Some((0.0, (1.0, 0.9))), Some((0.0, (1.0, 0.8))), Some((0.0, (1.0, 0.8)))]).collect();
        paired(&after_hiding, 2, false);
    }

    #[test]
    fn a_window_moving_every_frame_never_shows_an_image_through_texture_bounds_it_was_not_drawn_for() {
        let frames: Vec<Asked> = (0..40).map(|frame| Some((frame as f32 * 0.01, (1.0, [1.0, 0.9, 0.75, 0.9][frame / 7 % 4])))).collect();
        paired(&frames, 2, true);
    }

    #[test]
    fn a_window_moving_every_frame_keeps_showing_the_latest_image_and_hands_off_once_it_has_held() {
        let frames: Vec<Asked> = (0..20).map(|frame| Some((frame as f32 * 0.01, WHOLE))).chain([Some((0.19, WHOLE)); 4]).chain([Some((0.5, WHOLE)); 6]).collect();
        for (frame, shown) in composite(&frames, 0).iter().enumerate().take(20).skip(2) {
            assert_eq!(shown.iter().map(|&(drawn, _, _)| drawn).collect::<Vec<_>>(), [frame as u64], "frame {frame}");
        }
        paired(&frames, 21, false);
    }
}
