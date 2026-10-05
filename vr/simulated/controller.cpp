#include <openvr_driver.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>

using namespace vr;

static bool read_pose(const char *variable, DriverPose_t &pose) {
    const char *path = std::getenv(variable);
    FILE *file = path ? std::fopen(path, "r") : nullptr;
    if (!file) return false;
    double x, y, z, w = 1, qx = 0, qy = 0, qz = 0;
    int read = std::fscanf(file, "%lf %lf %lf %lf %lf %lf %lf", &x, &y, &z, &w, &qx, &qy, &qz);
    std::fclose(file);
    if (read != 3 && read != 7) return false;
    pose.vecPosition[0] = x;
    pose.vecPosition[1] = y;
    pose.vecPosition[2] = z;
    pose.qRotation = {w, qx, qy, qz};
    return true;
}

class Controller : public ITrackedDeviceServerDriver {
  public:
    const ETrackedControllerRole role;
    const char *const variable;
    uint32_t index = k_unTrackedDeviceIndexInvalid;

    Controller(ETrackedControllerRole role, const char *variable) : role(role), variable(variable) {}

    EVRInitError Activate(uint32_t id) override {
        index = id;
        PropertyContainerHandle_t container = VRProperties()->TrackedDeviceToPropertyContainer(id);
        VRProperties()->SetInt32Property(container, Prop_ControllerRoleHint_Int32, role);
        VRProperties()->SetStringProperty(container, Prop_ModelNumber_String, "simulated");
        VRProperties()->SetStringProperty(container, Prop_ControllerType_String, "simulated");
        return VRInitError_None;
    }
    void Deactivate() override { index = k_unTrackedDeviceIndexInvalid; }
    void EnterStandby() override {}
    void *GetComponent(const char *) override { return nullptr; }
    void DebugRequest(const char *, char *response, uint32_t size) override {
        if (size) response[0] = 0;
    }
    DriverPose_t GetPose() override {
        DriverPose_t pose = {};
        pose.qWorldFromDriverRotation.w = 1;
        pose.qDriverFromHeadRotation.w = 1;
        pose.deviceIsConnected = true;
        pose.poseIsValid = read_pose(variable, pose);
        pose.result = pose.poseIsValid ? TrackingResult_Running_OK : TrackingResult_Running_OutOfRange;
        return pose;
    }
};

class Provider : public IServerTrackedDeviceProvider {
    Controller left{TrackedControllerRole_LeftHand, "VOICE_VR_SIMULATED_LEFT"};
    Controller right{TrackedControllerRole_RightHand, "VOICE_VR_SIMULATED_RIGHT"};

  public:
    EVRInitError Init(IVRDriverContext *context) override {
        VR_INIT_SERVER_DRIVER_CONTEXT(context);
        VRServerDriverHost()->TrackedDeviceAdded("simulated-left", TrackedDeviceClass_Controller, &left);
        VRServerDriverHost()->TrackedDeviceAdded("simulated-right", TrackedDeviceClass_Controller, &right);
        return VRInitError_None;
    }
    void Cleanup() override { VR_CLEANUP_SERVER_DRIVER_CONTEXT(); }
    const char *const *GetInterfaceVersions() override { return k_InterfaceVersions; }
    void RunFrame() override {
        for (Controller *controller : {&left, &right}) {
            if (controller->index != k_unTrackedDeviceIndexInvalid) {
                DriverPose_t pose = controller->GetPose();
                VRServerDriverHost()->TrackedDevicePoseUpdated(controller->index, pose, sizeof pose);
            }
        }
        VREvent_t event;
        while (VRServerDriverHost()->PollNextEvent(&event, sizeof event)) {
        }
    }
    bool ShouldBlockStandbyMode() override { return false; }
    void EnterStandby() override {}
    void LeaveStandby() override {}
};

static Provider provider;

extern "C" __attribute__((visibility("default"))) void *HmdDriverFactory(const char *name, int *code) {
    if (std::strcmp(name, IServerTrackedDeviceProvider_Version) == 0) return &provider;
    if (code) *code = VRInitError_Init_InterfaceNotFound;
    return nullptr;
}
