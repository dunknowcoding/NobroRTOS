/* NobroRTOS managed camera lifecycle (header-only, no allocation).
 *
 * The camera facade (nobro::NiusCamAdapter) exposes bounded capture and a
 * one-shot recover(), and nobro_camera.h defines the DOWN/STARTING/READY/
 * SUSPENDED/FAULTED state enum -- but nothing drives that state machine, so a
 * sketch must hand-glue "capture failed -> decide to recover -> back off ->
 * re-init" itself. ManagedCamera unifies that: one poll() drives bounded
 * admission, fault detection, and re-init on an exponential backoff through a
 * start callback and a capture callback. It performs no allocation and calls
 * the callbacks only from poll(), never from an ISR. It is deliberately
 * transport-agnostic (callbacks, not a hard NiusCam dependency) so it is host
 * testable and works with any capture backend. */
#ifndef NOBRO_MANAGED_CAMERA_H
#define NOBRO_MANAGED_CAMERA_H

#include <stddef.h>
#include <stdint.h>

#include "nobro_camera.h"

namespace nobro {

/* Bring the sensor up (power-up / init / recover). Return true when ready. */
typedef bool (*CameraStart)(void *context);

/* Attempt one capture. On success, write the frame size to *out_bytes and
 * return true; return false when the capture failed (sensor glitch, DMA error,
 * absent device). */
typedef bool (*CameraCapture)(uint32_t *out_bytes, void *context);

/* Bounded per-window admission budget: no single frame over max_frame_bytes,
 * and no more than max_frames_per_window / max_bytes_per_window accepted before
 * resetWindow() is called. */
struct CameraFramePolicy {
    uint32_t max_frame_bytes;
    uint16_t max_frames_per_window;
    uint32_t max_bytes_per_window;

    static CameraFramePolicy make(uint32_t max_frame_bytes,
                                  uint16_t max_frames_per_window,
                                  uint32_t max_bytes_per_window) {
        CameraFramePolicy p;
        p.max_frame_bytes = max_frame_bytes;
        p.max_frames_per_window = max_frames_per_window;
        p.max_bytes_per_window = max_bytes_per_window;
        return p;
    }

    bool valid() const {
        return max_frame_bytes > 0 && max_frames_per_window > 0 &&
               max_bytes_per_window >= max_frame_bytes;
    }
};

/* Self-contained exponential backoff so a faulted camera is re-initialised on a
 * bounded schedule rather than in a hot retry loop. No cross-domain dependency. */
class CameraRecovery {
public:
    static CameraRecovery exponential(uint8_t max_attempts,
                                      uint64_t initial_backoff_us,
                                      uint64_t max_backoff_us) {
        CameraRecovery r;
        r.max_attempts_ = max_attempts;
        r.initial_us_ = initial_backoff_us;
        r.max_us_ = max_backoff_us;
        r.reset();
        return r;
    }

    bool valid() const {
        return max_attempts_ > 0 && initial_us_ > 0 && max_us_ >= initial_us_;
    }

    void reset() {
        failed_attempts_ = 0;
        next_attempt_us_ = 0;
        has_next_ = false;
    }

    /* Due when no attempt is scheduled yet, or the scheduled time has arrived,
     * and attempts are not exhausted. */
    bool ready(uint64_t now_us) const {
        if (failed_attempts_ >= max_attempts_) {
            return false;
        }
        return !has_next_ || now_us >= next_attempt_us_;
    }

    /* Record a failed attempt and arm the next backoff. Returns true when the
     * attempt budget is now exhausted. */
    bool failed(uint64_t now_us) {
        uint64_t backoff = initial_us_;
        for (uint8_t i = 0; i < failed_attempts_; ++i) {
            if (backoff >= max_us_ - backoff) {
                backoff = max_us_;
                break;
            }
            backoff += backoff;
            if (backoff >= max_us_) {
                backoff = max_us_;
                break;
            }
        }
        if (failed_attempts_ < 0xFF) {
            ++failed_attempts_;
        }
        next_attempt_us_ = saturatingAdd(now_us, backoff);
        has_next_ = true;
        return failed_attempts_ >= max_attempts_;
    }

    uint8_t failedAttempts() const { return failed_attempts_; }
    uint64_t nextAttemptUs() const { return next_attempt_us_; }
    bool exhausted() const { return failed_attempts_ >= max_attempts_; }

private:
    static uint64_t saturatingAdd(uint64_t a, uint64_t b) {
        return (a > UINT64_MAX - b) ? UINT64_MAX : a + b;
    }
    uint8_t max_attempts_;
    uint8_t failed_attempts_;
    uint64_t initial_us_;
    uint64_t max_us_;
    uint64_t next_attempt_us_;
    bool has_next_;
};

struct ManagedCameraEvent {
    nobro_camera_state_t state;   /* state after this poll */
    bool recovered;               /* became READY this poll */
    bool faulted;                 /* became FAULTED this poll */
    bool frame_captured;          /* a frame was accepted this poll */
    uint32_t frame_bytes;         /* size of the accepted frame, else 0 */
    uint64_t next_action_us;      /* when the next start attempt is due (down) */
};

/* Cohesive managed camera. poll() drives the whole lifecycle: when down or
 * faulted it re-inits on the recovery backoff via the start callback (never a
 * hot loop); when ready it admits one bounded, deadline-checked capture via the
 * capture callback and, on a capture failure, flips to FAULTED and rearms
 * recovery while preserving the diagnostics. */
class ManagedCamera {
public:
    ManagedCamera(const CameraFramePolicy &policy, const CameraRecovery &recovery)
        : policy_(policy), recovery_(recovery), state_(NOBRO_CAMERA_DOWN),
          frames_(0), bytes_(0), recoveries_(0), faults_(0), start_attempts_(0) {
        for (size_t i = 0; i < sizeof(diagnostics_); ++i) {
            reinterpret_cast<uint8_t *>(&diagnostics_)[i] = 0;
        }
    }

    bool valid() const { return policy_.valid() && recovery_.valid(); }
    nobro_camera_state_t state() const { return state_; }
    bool ready() const { return state_ == NOBRO_CAMERA_READY; }
    uint32_t recoveries() const { return recoveries_; }
    uint32_t faults() const { return faults_; }
    uint32_t startAttempts() const { return start_attempts_; }
    uint16_t framesThisWindow() const { return frames_; }
    const nobro_camera_diagnostics_t &diagnostics() const { return diagnostics_; }

    void resetWindow() {
        frames_ = 0;
        bytes_ = 0;
    }

    /* Force FAULTED and rearm recovery (e.g. the application observed a fault
     * the capture path did not surface). */
    void fault() {
        if (state_ != NOBRO_CAMERA_FAULTED) {
            ++faults_;
        }
        state_ = NOBRO_CAMERA_FAULTED;
        recovery_.reset();
    }

    ManagedCameraEvent poll(uint64_t now_us, uint64_t deadline_us,
                            CameraStart start, CameraCapture capture,
                            void *context = NULL) {
        ManagedCameraEvent ev;
        ev.recovered = false;
        ev.faulted = false;
        ev.frame_captured = false;
        ev.frame_bytes = 0;
        ev.next_action_us = now_us;
        if (!valid() || start == NULL || capture == NULL) {
            ev.state = state_;
            return ev;
        }

        if (state_ == NOBRO_CAMERA_DOWN || state_ == NOBRO_CAMERA_FAULTED) {
            if (recovery_.ready(now_us)) {
                ++start_attempts_;
                state_ = NOBRO_CAMERA_STARTING;
                if (start(context)) {
                    state_ = NOBRO_CAMERA_READY;
                    recovery_.reset();
                    ++recoveries_;
                    diagnostics_.recoveries++;
                    ev.recovered = true;
                    ev.state = state_;
                    return ev;
                }
                recovery_.failed(now_us);
                state_ = NOBRO_CAMERA_FAULTED;
            }
            ev.state = state_;
            ev.next_action_us = recovery_.nextAttemptUs();
            return ev;
        }

        /* READY: one bounded, deadline-checked capture. */
        if (now_us > deadline_us) {
            diagnostics_.deadline_rejections++;
            diagnostics_.frames_dropped++;
            ev.state = state_;
            return ev;
        }
        if (frames_ >= policy_.max_frames_per_window ||
            bytes_ >= policy_.max_bytes_per_window) {
            diagnostics_.memory_rejections++;
            diagnostics_.frames_dropped++;
            ev.state = state_;
            return ev;
        }
        uint32_t size = 0;
        if (!capture(&size, context)) {
            diagnostics_.capture_failures++;
            diagnostics_.frames_dropped++;
            state_ = NOBRO_CAMERA_FAULTED;
            recovery_.reset();
            ++faults_;
            ev.faulted = true;
            ev.state = state_;
            ev.next_action_us = now_us; /* recovery attempt is due immediately */
            return ev;
        }
        if (size == 0 || size > policy_.max_frame_bytes ||
            size > policy_.max_bytes_per_window - bytes_) {
            diagnostics_.memory_rejections++;
            diagnostics_.frames_dropped++;
            ev.state = state_;
            return ev;
        }
        ++frames_;
        bytes_ += size;
        diagnostics_.frames_accepted++;
        diagnostics_.bytes_accepted += static_cast<uint64_t>(size);
        ev.frame_captured = true;
        ev.frame_bytes = size;
        ev.state = state_;
        return ev;
    }

private:
    CameraFramePolicy policy_;
    CameraRecovery recovery_;
    nobro_camera_state_t state_;
    uint16_t frames_;
    uint32_t bytes_;
    uint32_t recoveries_;
    uint32_t faults_;
    uint32_t start_attempts_;
    nobro_camera_diagnostics_t diagnostics_;
};

}  // namespace nobro

#endif
