#ifndef NOBRO_NIUS_DISPLAY_H
#define NOBRO_NIUS_DISPLAY_H

#include "nobro_display.h"

#if !defined(NOBRO_DISPLAY_DISABLED)

#include <NiusDisplay.h>

namespace nobro {

// Fixed-capacity, no-heap RGB565 queue for any pixel-addressable NiusSurface.
// Submitted payloads are caller-owned and must remain unchanged until their
// receipt becomes terminal or is cancelled.
template <size_t Slots>
class NiusDisplayAdapter {
public:
    explicit NiusDisplayAdapter(NiusSurface &surface)
        : surface_(surface), state_(NOBRO_DISPLAY_DOWN), nextSequence_(0),
          queued_(0) {
        for (size_t i = 0; i < storageSlots(); ++i) frames_[i].used = false;
    }

    nobro_display_result_t begin() {
        if (state_ != NOBRO_DISPLAY_DOWN || Slots == 0 || !surface_.ready())
            return NOBRO_DISPLAY_INVALID_CONFIG;
        state_ = NOBRO_DISPLAY_READY;
        return NOBRO_DISPLAY_OK;
    }

    nobro_display_result_t submit(nobro_display_region_t region,
                                  const uint8_t *rgb565, size_t bytes,
                                  uint32_t deadline_us,
                                  nobro_display_receipt_t *out) {
        if (state_ != NOBRO_DISPLAY_READY) return NOBRO_DISPLAY_NOT_READY;
        if (!validRegion(region)) return NOBRO_DISPLAY_INVALID_REGION;
        const size_t expected = static_cast<size_t>(region.width) * region.height * 2U;
        if (!rgb565 || bytes != expected || !out) return NOBRO_DISPLAY_INVALID_PAYLOAD;
        Frame *frame = freeFrame();
        if (!frame) return NOBRO_DISPLAY_BACKPRESSURED;
        frame->used = true;
        frame->pixels = rgb565;
        frame->receipt = {};
        frame->receipt.version = NOBRO_DISPLAY_RECEIPT_VERSION;
        frame->receipt.sequence = nextSequence();
        frame->receipt.region = region;
        frame->receipt.payload_bytes = static_cast<uint32_t>(bytes);
        frame->receipt.payload_checksum = checksum(rgb565, bytes);
        frame->receipt.submitted_us = micros();
        frame->receipt.deadline_us = deadline_us;
        frame->receipt.status = NOBRO_DISPLAY_FRAME_QUEUED;
        ++queued_;
        *out = frame->receipt;
        return NOBRO_DISPLAY_OK;
    }

    nobro_display_result_t pump() {
        if (state_ != NOBRO_DISPLAY_READY) return NOBRO_DISPLAY_NOT_READY;
        Frame *frame = oldestFrame();
        if (!frame) return NOBRO_DISPLAY_OK;
        const uint32_t started = micros();
        if (frame->receipt.deadline_us != 0 &&
            static_cast<int32_t>(started - frame->receipt.deadline_us) > 0) {
            finish(*frame, NOBRO_DISPLAY_FRAME_DEADLINE_MISS, started);
            return NOBRO_DISPLAY_DEADLINE_MISS;
        }
        state_ = NOBRO_DISPLAY_BUSY;
        const nobro_display_region_t &r = frame->receipt.region;
        size_t offset = 0;
        for (uint16_t y = 0; y < r.height; ++y) {
            for (uint16_t x = 0; x < r.width; ++x) {
                const uint16_t value =
                    (static_cast<uint16_t>(frame->pixels[offset]) << 8) |
                    frame->pixels[offset + 1U];
                offset += 2U;
                surface_.drawPixel(
                    static_cast<nd_coord>(r.x + x), static_cast<nd_coord>(r.y + y),
                    NIUS_RGB(((value >> 11) & 0x1fU) * 255U / 31U,
                             ((value >> 5) & 0x3fU) * 255U / 63U,
                             (value & 0x1fU) * 255U / 31U));
            }
        }
        if (surface_.display() != ND_OK) {
            state_ = NOBRO_DISPLAY_FAULTED;
            finish(*frame, NOBRO_DISPLAY_FRAME_TRANSPORT_ERROR, micros());
            return NOBRO_DISPLAY_TRANSPORT_ERROR;
        }
        const uint32_t completed = micros();
        state_ = NOBRO_DISPLAY_READY;
        if (frame->receipt.deadline_us != 0 &&
            static_cast<int32_t>(completed - frame->receipt.deadline_us) > 0) {
            finish(*frame, NOBRO_DISPLAY_FRAME_DEADLINE_MISS, completed);
            return NOBRO_DISPLAY_DEADLINE_MISS;
        }
        finish(*frame, NOBRO_DISPLAY_FRAME_COMPLETE, completed);
        return NOBRO_DISPLAY_OK;
    }

    nobro_display_result_t cancel(uint32_t sequence) {
        Frame *frame = find(sequence);
        if (!frame) return NOBRO_DISPLAY_CANCELLED;
        finish(*frame, NOBRO_DISPLAY_FRAME_CANCELLED, micros());
        return NOBRO_DISPLAY_OK;
    }

    bool receipt(uint32_t sequence, nobro_display_receipt_t *out) const {
        if (!out) return false;
        for (size_t i = 0; i < storageSlots(); ++i) {
            if (history_[i].sequence == sequence) { *out = history_[i]; return true; }
            if (frames_[i].used && frames_[i].receipt.sequence == sequence) {
                *out = frames_[i].receipt; return true;
            }
        }
        return false;
    }

    void quiesce() { clear(); state_ = NOBRO_DISPLAY_SUSPENDED; }
    nobro_display_result_t recover() {
        clear();
        if (!surface_.ready()) { state_ = NOBRO_DISPLAY_FAULTED; return NOBRO_DISPLAY_TRANSPORT_ERROR; }
        state_ = NOBRO_DISPLAY_READY;
        return NOBRO_DISPLAY_OK;
    }
    void release() { clear(); state_ = NOBRO_DISPLAY_DOWN; }
    size_t queued() const { return queued_; }
    size_t capacity() const { return Slots; }
    size_t staticRamBytes() const { return sizeof(*this); }
    nobro_display_state_t state() const { return state_; }

private:
    struct Frame {
        const uint8_t *pixels;
        nobro_display_receipt_t receipt;
        bool used;
    };

    static size_t storageSlots() { return Slots == 0 ? 1 : Slots; }
    bool validRegion(nobro_display_region_t r) const {
        return r.width && r.height && r.x < static_cast<uint16_t>(surface_.width())
            && r.y < static_cast<uint16_t>(surface_.height())
            && static_cast<uint32_t>(r.x) + r.width <= static_cast<uint16_t>(surface_.width())
            && static_cast<uint32_t>(r.y) + r.height <= static_cast<uint16_t>(surface_.height());
    }
    uint32_t nextSequence() { if (++nextSequence_ == 0) ++nextSequence_; return nextSequence_; }
    static uint32_t checksum(const uint8_t *data, size_t size) {
        uint32_t hash = 0x811c9dc5UL;
        for (size_t i = 0; i < size; ++i) { hash ^= data[i]; hash *= 0x01000193UL; }
        return hash;
    }
    Frame *freeFrame() {
        for (size_t i = 0; i < storageSlots(); ++i) if (!frames_[i].used) return &frames_[i];
        return 0;
    }
    Frame *oldestFrame() {
        Frame *oldest = 0;
        for (size_t i = 0; i < storageSlots(); ++i)
            if (frames_[i].used && (!oldest || frames_[i].receipt.sequence < oldest->receipt.sequence)) oldest = &frames_[i];
        return oldest;
    }
    Frame *find(uint32_t sequence) {
        for (size_t i = 0; i < storageSlots(); ++i)
            if (frames_[i].used && frames_[i].receipt.sequence == sequence) return &frames_[i];
        return 0;
    }
    void finish(Frame &frame, nobro_display_frame_status_t status, uint32_t now) {
        frame.receipt.status = status;
        frame.receipt.completed_us = now ? now : 1U;
        const size_t slot = frame.receipt.sequence % storageSlots();
        history_[slot] = frame.receipt;
        frame.used = false;
        frame.pixels = 0;
        --queued_;
    }
    void clear() {
        for (size_t i = 0; i < storageSlots(); ++i) frames_[i].used = false;
        queued_ = 0;
    }

    NiusSurface &surface_;
    Frame frames_[Slots == 0 ? 1 : Slots];
    nobro_display_receipt_t history_[Slots == 0 ? 1 : Slots]{};
    nobro_display_state_t state_;
    uint32_t nextSequence_;
    size_t queued_;
};

}  // namespace nobro

#endif
#endif
