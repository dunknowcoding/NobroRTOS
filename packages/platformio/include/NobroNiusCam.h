#ifndef NOBRO_NIUS_CAM_H
#define NOBRO_NIUS_CAM_H

#include <freertos/FreeRTOS.h>
#include <NiusCam.h>
#include "nobro_camera.h"

namespace nobro {

class NiusCamAdapter {
public:
    static constexpr const char *backendId() { return "niuscam-arduino"; }

    NiusCamAdapter(NiusCam::Camera &camera, uint32_t maxFrameBytes,
                   uint16_t maxFramesPerWindow, uint32_t maxBytesPerWindow,
                   uint8_t maxInFlight = 1, uint16_t instanceId = 0)
        : camera_(camera), maxFrameBytes_(maxFrameBytes),
          maxFrames_(maxFramesPerWindow), maxBytes_(maxBytesPerWindow),
          maxInFlight_(maxInFlight), frames_(0), bytes_(0), inFlight_(0),
          instanceId_(instanceId), generation_(0), mounted_(false), suspended_(false),
          diagnostics_{} {}

    ~NiusCamAdapter() { releaseMount(); }

    NiusCamAdapter(const NiusCamAdapter &) = delete;
    NiusCamAdapter &operator=(const NiusCamAdapter &) = delete;

    bool begin(const NiusCam::BoardProfile &board,
               const NiusCam::Config &config = NiusCam::Config::Balanced()) {
        if (!mounted_ && !claimMount()) return false;
        suspended_ = false;
        if (camera_.begin(board, config).ok()) {
            if (generation_ == 0) generation_ = 1;
            return true;
        }
        releaseMount();
        return false;
    }

    uint16_t instanceId() const { return instanceId_; }
    uint32_t generation() const { return generation_; }
    bool mounted() const { return mounted_; }

    NiusCam::Frame capture(uint64_t nowUs, uint64_t deadlineUs) {
        if (suspended_ || !camera_.isReady()) return rejectCapture();
        if (nowUs > deadlineUs) {
            diagnostics_.deadline_rejections++;
            return rejectCapture();
        }
        if (frames_ >= maxFrames_ || bytes_ >= maxBytes_) {
            diagnostics_.memory_rejections++;
            return rejectCapture();
        }
        if (inFlight_ >= maxInFlight_) {
            diagnostics_.backpressure_rejections++;
            return rejectCapture();
        }
        NiusCam::Frame frame = camera_.capture();
        if (!frame) {
            diagnostics_.capture_failures++;
            return rejectCapture();
        }
        const size_t size = frame.size();
        if (size > maxFrameBytes_ || size > UINT32_MAX || bytes_ > maxBytes_ ||
            static_cast<uint32_t>(size) > maxBytes_ - bytes_) {
            diagnostics_.memory_rejections++;
            return rejectCapture();
        }
        frames_++;
        bytes_ += static_cast<uint32_t>(size);
        inFlight_++;
        diagnostics_.frames_accepted++;
        diagnostics_.bytes_accepted += static_cast<uint64_t>(size);
        return frame;
    }

    void release(NiusCam::Frame &frame) {
        frame.release();
        if (inFlight_ > 0) inFlight_--;
    }

    void resetWindow() { frames_ = 0; bytes_ = 0; }

    // Fail-closed software suspend: lifecycle parity with the audio quiesce()
    // and IMU suspend() hooks. A quiesced adapter rejects capture() regardless
    // of module state until recover() clears it. This is an honest software
    // gate, not a claimed sensor power-down.
    void quiesce() { suspended_ = true; }
    bool quiesced() const { return suspended_; }

    bool recover() {
        if (!camera_.recover().ok()) return false;
        suspended_ = false;
        if (generation_ != UINT32_MAX) generation_++;
        diagnostics_.recoveries++;
        return true;
    }

    nobro_camera_diagnostics_t diagnostics() const { return diagnostics_; }

private:
    struct MountSlot {
        NiusCam::Camera *camera;
        uint16_t instance;
    };

    static MountSlot *mountSlots() {
        static MountSlot slots[4] = {};
        return slots;
    }

    static portMUX_TYPE *mountMux() {
        static portMUX_TYPE mux = portMUX_INITIALIZER_UNLOCKED;
        return &mux;
    }

    bool claimMount() {
        portENTER_CRITICAL(mountMux());
        MountSlot *slots = mountSlots();
        for (uint8_t index = 0; index < 4; ++index) {
            if (slots[index].camera == &camera_ ||
                (slots[index].camera != nullptr &&
                 slots[index].instance == instanceId_)) {
                portEXIT_CRITICAL(mountMux());
                return false;
            }
        }
        for (uint8_t index = 0; index < 4; ++index) {
            if (slots[index].camera == nullptr) {
                slots[index].camera = &camera_;
                slots[index].instance = instanceId_;
                mounted_ = true;
                portEXIT_CRITICAL(mountMux());
                return true;
            }
        }
        portEXIT_CRITICAL(mountMux());
        return false;
    }

    void releaseMount() {
        if (!mounted_) return;
        portENTER_CRITICAL(mountMux());
        MountSlot *slots = mountSlots();
        for (uint8_t index = 0; index < 4; ++index) {
            if (slots[index].camera == &camera_ &&
                slots[index].instance == instanceId_) {
                slots[index].camera = nullptr;
                slots[index].instance = 0;
                break;
            }
        }
        mounted_ = false;
        portEXIT_CRITICAL(mountMux());
    }

    NiusCam::Frame rejectCapture() {
        diagnostics_.frames_dropped++;
        return NiusCam::Frame();
    }

    NiusCam::Camera &camera_;
    uint32_t maxFrameBytes_;
    uint16_t maxFrames_;
    uint32_t maxBytes_;
    uint8_t maxInFlight_;
    uint16_t frames_;
    uint32_t bytes_;
    uint8_t inFlight_;
    uint16_t instanceId_;
    uint32_t generation_;
    bool mounted_;
    bool suspended_;
    nobro_camera_diagnostics_t diagnostics_;
};

}  // namespace nobro

#endif
