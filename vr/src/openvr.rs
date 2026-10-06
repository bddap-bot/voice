use openvr_sys as sys;
use std::ffi::{CStr, CString};

use crate::placement::{Hand, HandPose, Pose, Vec3};

pub struct Runtime {
    system: &'static sys::VR_IVRSystem_FnTable,
    overlay: &'static sys::VR_IVROverlay_FnTable,
    compositor: &'static sys::VR_IVRCompositor_FnTable,
    render_models: &'static sys::VR_IVRRenderModels_FnTable,
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
        Ok(Runtime { system: table(sys::IVRSystem_Version)?, overlay: table(sys::IVROverlay_Version)?, compositor: table(sys::IVRCompositor_Version)?, render_models: table(sys::IVRRenderModels_Version)? })
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

    pub fn hand_index(&self, hand: Hand) -> Option<u32> {
        let role = match hand {
            Hand::Left => sys::ETrackedControllerRole_TrackedControllerRole_LeftHand,
            Hand::Right => sys::ETrackedControllerRole_TrackedControllerRole_RightHand,
        };
        let index = call!(self.system, GetTrackedDeviceIndexForControllerRole, role);
        (index < sys::k_unMaxTrackedDeviceCount as u32).then_some(index)
    }

    pub fn tip(&self, device: u32) -> Result<Vec3, String> {
        let mut buffer = vec![0 as std::os::raw::c_char; 256];
        let mut error = 0;
        call!(self.system, GetStringTrackedDeviceProperty, device, sys::ETrackedDeviceProperty_Prop_RenderModelName_String, buffer.as_mut_ptr(), buffer.len() as u32, &mut error);
        if error != 0 {
            return Err(format!("device {device} has no render model name (property error {error})"));
        }
        let model = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_owned();
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
        Ok(Overlay { runtime: self, handle, shown: false })
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe { sys::VR_ShutdownInternal() };
    }
}

fn split(buffer: &[std::os::raw::c_char]) -> Vec<CString> {
    unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().split_whitespace().map(|name| CString::new(name).unwrap()).collect()
}

pub struct Overlay<'a> {
    runtime: &'a Runtime,
    handle: sys::VROverlayHandle_t,
    shown: bool,
}

impl Overlay<'_> {
    pub fn place_on(&self, device: u32, offset: &Pose) {
        let mut matrix = sys::HmdMatrix34_t { m: offset.to_m34() };
        call!(self.runtime.overlay, SetOverlayTransformTrackedDeviceRelative, self.handle, device, &mut matrix);
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
