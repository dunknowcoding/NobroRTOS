#ifndef NOBRO_NIUS_CAM_H
#define NOBRO_NIUS_CAM_H

#include <NiusCam.h>
#include "nobro_camera.h"

namespace nobro {

class NiusCamAdapter {
public:
    NiusCamAdapter(NiusCam::Camera &camera, uint32_t maxFrameBytes,
                   uint16_t maxFramesPerWindow, uint32_t maxBytesPerWindow,
                   uint8_t maxInFlight = 1)
        : camera_(camera), maxFrameBytes_(maxFrameBytes),
          maxFrames_(maxFramesPerWindow), maxBytes_(maxBytesPerWindow),
          maxInFlight_(maxInFlight), frames_(0), bytes_(0), inFlight_(0),
          diagnostics_{} {}

    bool begin(const NiusCam::BoardProfile &board,
               const NiusCam::Config &config = NiusCam::Config::Balanced()) {
        return camera_.begin(board, config).ok();
    }

    NiusCam::Frame capture(uint64_t nowUs, uint64_t deadlineUs) {
        if (!camera_.isReady()) return rejectCapture();
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

    bool recover() {
        if (!camera_.recover().ok()) return false;
        diagnostics_.recoveries++;
        return true;
    }

    nobro_camera_diagnostics_t diagnostics() const { return diagnostics_; }

private:
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
    nobro_camera_diagnostics_t diagnostics_;
};

}  // namespace nobro

#endif
