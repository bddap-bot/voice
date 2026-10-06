#include <vulkan/vulkan.h>
#include <openvr/openvr.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>
#include <sstream>
#include <chrono>
#define CK(x) do { VkResult r_ = (x); if (r_ != VK_SUCCESS) { fprintf(stderr, "%s -> %d\n", #x, r_); exit(2); } } while (0)

static std::vector<std::string> split(const char* s) {
  std::vector<std::string> v; std::istringstream is(s); std::string w; while (is >> w) v.push_back(w); return v;
}
struct Img { VkImage img; VkDeviceMemory mem; uint32_t w, h; };
static VkDevice dev; static VkPhysicalDevice phys;
static Img makeImg(uint32_t w, uint32_t h) {
  Img r{}; r.w = w; r.h = h;
  VkImageCreateInfo ci{VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO};
  ci.imageType = VK_IMAGE_TYPE_2D; ci.format = VK_FORMAT_R8G8B8A8_SRGB; ci.extent = {w, h, 1};
  ci.mipLevels = 1; ci.arrayLayers = 1; ci.samples = VK_SAMPLE_COUNT_1_BIT; ci.tiling = VK_IMAGE_TILING_OPTIMAL;
  ci.usage = VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
  CK(vkCreateImage(dev, &ci, nullptr, &r.img));
  VkMemoryRequirements mr; vkGetImageMemoryRequirements(dev, r.img, &mr);
  VkPhysicalDeviceMemoryProperties mp; vkGetPhysicalDeviceMemoryProperties(phys, &mp);
  uint32_t ti = 0; for (; ti < mp.memoryTypeCount; ti++) if ((mr.memoryTypeBits & (1u << ti)) && (mp.memoryTypes[ti].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)) break;
  VkMemoryAllocateInfo ai{VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO}; ai.allocationSize = mr.size; ai.memoryTypeIndex = ti;
  CK(vkAllocateMemory(dev, &ai, nullptr, &r.mem)); CK(vkBindImageMemory(dev, r.img, r.mem, 0));
  return r;
}
static void clearTo(VkCommandBuffer cb, const Img& i, float r, float g, float b) {
  VkImageSubresourceRange rng{VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
  VkImageMemoryBarrier br{VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER};
  br.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED; br.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL; br.image = i.img; br.subresourceRange = rng;
  br.srcQueueFamilyIndex = br.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; br.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
  vkCmdPipelineBarrier(cb, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, nullptr, 0, nullptr, 1, &br);
  VkClearColorValue c{}; c.float32[0] = r; c.float32[1] = g; c.float32[2] = b; c.float32[3] = 1;
  vkCmdClearColorImage(cb, i.img, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, &c, 1, &rng);
  br.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL; br.newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
  br.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT; br.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
  vkCmdPipelineBarrier(cb, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, nullptr, 0, nullptr, 1, &br);
}
int main(int argc, char** argv) {
  int secs = argc > 1 ? atoi(argv[1]) : 20;
  vr::EVRInitError e;
  vr::VR_Init(&e, vr::VRApplication_Scene);
  if (e) { fprintf(stderr, "VR_Init: %s\n", vr::VR_GetVRInitErrorAsEnglishDescription(e)); return 1; }
  char buf[4096];
  vr::VRCompositor()->GetVulkanInstanceExtensionsRequired(buf, sizeof buf);
  auto iext = split(buf); std::vector<const char*> ip; for (auto& s : iext) ip.push_back(s.c_str());
  VkApplicationInfo app{VK_STRUCTURE_TYPE_APPLICATION_INFO}; app.apiVersion = VK_API_VERSION_1_1;
  VkInstanceCreateInfo ici{VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO}; ici.pApplicationInfo = &app;
  ici.enabledExtensionCount = ip.size(); ici.ppEnabledExtensionNames = ip.data();
  VkInstance inst; CK(vkCreateInstance(&ici, nullptr, &inst));
  uint64_t out = 0; vr::VRSystem()->GetOutputDevice(&out, vr::TextureType_Vulkan, inst);
  phys = (VkPhysicalDevice)out;
  if (!phys) { uint32_t n = 1; vkEnumeratePhysicalDevices(inst, &n, &phys); }
  VkPhysicalDeviceProperties pp; vkGetPhysicalDeviceProperties(phys, &pp);
  printf("vk device: %s (OpenVR output device %s)\n", pp.deviceName, out ? "matched" : "not reported");
  vr::VRCompositor()->GetVulkanDeviceExtensionsRequired(phys, buf, sizeof buf);
  auto dext = split(buf); std::vector<const char*> dp; for (auto& s : dext) dp.push_back(s.c_str());
  printf("device exts required: %s\n", buf);
  uint32_t nq; vkGetPhysicalDeviceQueueFamilyProperties(phys, &nq, nullptr);
  std::vector<VkQueueFamilyProperties> qf(nq); vkGetPhysicalDeviceQueueFamilyProperties(phys, &nq, qf.data());
  uint32_t qfi = 0; while (!(qf[qfi].queueFlags & VK_QUEUE_GRAPHICS_BIT)) qfi++;
  float prio = 1; VkDeviceQueueCreateInfo qci{VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO}; qci.queueFamilyIndex = qfi; qci.queueCount = 1; qci.pQueuePriorities = &prio;
  VkDeviceCreateInfo dci{VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO}; dci.queueCreateInfoCount = 1; dci.pQueueCreateInfos = &qci;
  dci.enabledExtensionCount = dp.size(); dci.ppEnabledExtensionNames = dp.data();
  CK(vkCreateDevice(phys, &dci, nullptr, &dev));
  VkQueue q; vkGetDeviceQueue(dev, qfi, 0, &q);
  uint32_t rw, rh; vr::VRSystem()->GetRecommendedRenderTargetSize(&rw, &rh);
  Img eyeL = makeImg(rw, rh), eyeR = makeImg(rw, rh), ov = makeImg(256, 256);
  VkCommandPoolCreateInfo pci{VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO}; pci.queueFamilyIndex = qfi;
  VkCommandPool pool; CK(vkCreateCommandPool(dev, &pci, nullptr, &pool));
  VkCommandBufferAllocateInfo cai{VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO}; cai.commandPool = pool; cai.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY; cai.commandBufferCount = 1;
  VkCommandBuffer cb; CK(vkAllocateCommandBuffers(dev, &cai, &cb));
  VkCommandBufferBeginInfo bi{VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO}; CK(vkBeginCommandBuffer(cb, &bi));
  clearTo(cb, eyeL, 0.05f, 0.15f, 0.6f); clearTo(cb, eyeR, 0.05f, 0.15f, 0.6f); clearTo(cb, ov, 1.0f, 0.5f, 0.0f);
  CK(vkEndCommandBuffer(cb));
  VkSubmitInfo si{VK_STRUCTURE_TYPE_SUBMIT_INFO}; si.commandBufferCount = 1; si.pCommandBuffers = &cb;
  CK(vkQueueSubmit(q, 1, &si, VK_NULL_HANDLE)); CK(vkQueueWaitIdle(q));
  auto vkt = [&](const Img& i) { vr::VRVulkanTextureData_t t{}; t.m_nImage = (uint64_t)i.img; t.m_pDevice = dev; t.m_pPhysicalDevice = phys;
    t.m_pInstance = inst; t.m_pQueue = q; t.m_nQueueFamilyIndex = qfi; t.m_nWidth = i.w; t.m_nHeight = i.h; t.m_nFormat = VK_FORMAT_R8G8B8A8_SRGB; t.m_nSampleCount = 1; return t; };
  vr::VRVulkanTextureData_t tl = vkt(eyeL), tr = vkt(eyeR), to = vkt(ov);
  vr::VROverlayHandle_t h;
  printf("CreateOverlay %d\n", vr::VROverlay()->CreateOverlay("vrh.vk", "vrh vulkan", &h));
  vr::Texture_t otex{&to, vr::TextureType_Vulkan, vr::ColorSpace_Auto};
  printf("SetOverlayTexture(Vulkan) %d\n", vr::VROverlay()->SetOverlayTexture(h, &otex));
  vr::VROverlay()->SetOverlayWidthInMeters(h, 0.3f);
  vr::HmdMatrix34_t m = {{{1,0,0,0.45f},{0,1,0,0},{0,0,1,-1.0f}}};
  vr::VROverlay()->SetOverlayTransformTrackedDeviceRelative(h, vr::k_unTrackedDeviceIndex_Hmd, &m);
  printf("ShowOverlay %d\n", vr::VROverlay()->ShowOverlay(h)); fflush(stdout);
  vr::TrackedDevicePose_t poses[vr::k_unMaxTrackedDeviceCount];
  auto t0 = std::chrono::steady_clock::now(); int frames = 0, errs = 0;
  vr::Texture_t el{&tl, vr::TextureType_Vulkan, vr::ColorSpace_Auto}, er{&tr, vr::TextureType_Vulkan, vr::ColorSpace_Auto};
  while (std::chrono::steady_clock::now() - t0 < std::chrono::seconds(secs)) {
    vr::VRCompositor()->WaitGetPoses(poses, vr::k_unMaxTrackedDeviceCount, nullptr, 0);
    if (vr::VRCompositor()->Submit(vr::Eye_Left, &el)) errs++;
    if (vr::VRCompositor()->Submit(vr::Eye_Right, &er)) errs++;
    frames++;
  }
  printf("frames=%d submit_errors=%d\n", frames, errs);
  vkDeviceWaitIdle(dev);
  vr::VR_Shutdown();
  return 0;
}
