#ifndef NOBRO_NIUS_AUDIO_H
#define NOBRO_NIUS_AUDIO_H

#include "nobro_audio.h"

#if !defined(NOBRO_AUDIO_DISABLED)

#include <NiusAudio.h>

namespace nobro {

// Bounded Nobro frame/deadline/backpressure facade for the qualified
// NiusAudio WeAct ES8311 + NS4150B path. The caller owns the module object,
// wiring, and lifetime. No heap allocation occurs in this adapter.
template <size_t Slots, size_t SamplesPerFrame>
class NiusEs8311AudioAdapter {
public:
    explicit NiusEs8311AudioAdapter(NiusAudioWeActEs8311Board &module)
        : module_(module), read_(0), write_(0), queued_(0),
          state_(NOBRO_AUDIO_DOWN), diagnostics_{} {
        for (size_t index = 0; index < storageSlots(); ++index) {
            frames_[index].samples = 0;
        }
    }

    nobro_audio_result_t configure(
        int8_t shutdown_pin, int8_t sda_pin, int8_t scl_pin, int8_t mclk_pin,
        int8_t bclk_pin, int8_t codec_dout_pin, int8_t ws_pin,
        int8_t codec_din_pin, const nobro_audio_format_t &format) {
        if (state_ != NOBRO_AUDIO_DOWN || !validFormat(format)) {
            return NOBRO_AUDIO_INVALID_CONFIG;
        }
        module_.setControlPins(
            shutdown_pin, sda_pin, scl_pin, mclk_pin);
        module_.setAudioPins(
            bclk_pin, codec_dout_pin, ws_pin, codec_din_pin);
        module_.setFormat(
            format.sample_rate_hz, format.bits_per_sample, format.channels);
        format_ = format;
        return NOBRO_AUDIO_OK;
    }

    nobro_audio_result_t begin() {
        if (state_ != NOBRO_AUDIO_DOWN || !validFormat(format_)) {
            return NOBRO_AUDIO_INVALID_CONFIG;
        }
        if (!module_.begin() || !module_.beginAudio()) {
            state_ = NOBRO_AUDIO_FAULTED;
            increment(diagnostics_.transport_errors);
            return NOBRO_AUDIO_TRANSPORT_ERROR;
        }
        state_ = NOBRO_AUDIO_READY;
        return NOBRO_AUDIO_OK;
    }

    nobro_audio_result_t submit(const int16_t *samples, size_t sample_count) {
        if (state_ != NOBRO_AUDIO_READY) {
            return NOBRO_AUDIO_NOT_READY;
        }
        if (samples == 0 || sample_count == 0 ||
            sample_count > SamplesPerFrame ||
            sample_count > format_.samples_per_frame) {
            increment(diagnostics_.oversized_rejections);
            return NOBRO_AUDIO_FRAME_TOO_LARGE;
        }
        if (Slots == 0 || queued_ == Slots) {
            increment(diagnostics_.backpressure_rejections);
            return NOBRO_AUDIO_BACKPRESSURED;
        }
        Frame &frame = frames_[write_];
        for (size_t index = 0; index < sample_count; ++index) {
            frame.data[index] = samples[index];
        }
        frame.samples = sample_count;
        write_ = (write_ + 1U) % storageSlots();
        ++queued_;
        return NOBRO_AUDIO_OK;
    }

    // Sends at most one queued frame. max_block_us is a measured completion
    // budget: a non-preemptible vendor call that exceeds it is recorded and
    // reported rather than represented as a hard real-time guarantee.
    nobro_audio_result_t pump(uint32_t max_block_us) {
        if (state_ != NOBRO_AUDIO_READY) {
            return NOBRO_AUDIO_NOT_READY;
        }
        if (queued_ == 0) {
            return NOBRO_AUDIO_OK;
        }
        Frame &frame = frames_[read_];
        const uint32_t started = micros();
        const size_t sent = module_.writeSamples(frame.data, frame.samples);
        const uint32_t elapsed = static_cast<uint32_t>(micros() - started);
        if (sent != frame.samples) {
            state_ = NOBRO_AUDIO_FAULTED;
            increment(diagnostics_.partial_transfers);
            increment(diagnostics_.transport_errors);
            return NOBRO_AUDIO_PARTIAL_IO;
        }
        increment(diagnostics_.playback_frames);
        add(diagnostics_.samples_played, sent);
        frame.samples = 0;
        read_ = (read_ + 1U) % storageSlots();
        --queued_;
        if (max_block_us != 0 && elapsed > max_block_us) {
            increment(diagnostics_.deadline_misses);
            return NOBRO_AUDIO_DEADLINE_MISS;
        }
        return NOBRO_AUDIO_OK;
    }

    nobro_audio_result_t capture(int16_t *samples, size_t sample_count,
                                 uint32_t max_block_us) {
        if (state_ != NOBRO_AUDIO_READY) {
            return NOBRO_AUDIO_NOT_READY;
        }
        if (samples == 0 || sample_count == 0 ||
            sample_count > SamplesPerFrame ||
            sample_count > format_.samples_per_frame) {
            increment(diagnostics_.oversized_rejections);
            return NOBRO_AUDIO_FRAME_TOO_LARGE;
        }
        const uint32_t started = micros();
        const size_t received = module_.readSamples(samples, sample_count);
        const uint32_t elapsed = static_cast<uint32_t>(micros() - started);
        if (received != sample_count) {
            state_ = NOBRO_AUDIO_FAULTED;
            increment(diagnostics_.partial_transfers);
            increment(diagnostics_.transport_errors);
            return NOBRO_AUDIO_PARTIAL_IO;
        }
        increment(diagnostics_.capture_frames);
        add(diagnostics_.samples_captured, received);
        if (max_block_us != 0 && elapsed > max_block_us) {
            increment(diagnostics_.deadline_misses);
            return NOBRO_AUDIO_DEADLINE_MISS;
        }
        return NOBRO_AUDIO_OK;
    }

    void quiesce() {
        if (state_ == NOBRO_AUDIO_DOWN || state_ == NOBRO_AUDIO_SUSPENDED) {
            clearQueue();
            return;
        }
        module_.endAudio();
        clearQueue();
        state_ = NOBRO_AUDIO_SUSPENDED;
    }

    nobro_audio_result_t recover() {
        module_.endAudio();
        clearQueue();
        state_ = NOBRO_AUDIO_DOWN;
        const nobro_audio_result_t result = begin();
        if (result == NOBRO_AUDIO_OK) {
            increment(diagnostics_.recoveries);
        }
        return result;
    }

    size_t queuedFrames() const { return queued_; }
    size_t capacity() const { return Slots; }
    size_t staticRamBytes() const { return sizeof(*this); }
    nobro_audio_state_t state() const { return state_; }
    nobro_audio_diagnostics_t diagnostics() const { return diagnostics_; }
    const char *lastMessage() const { return module_.lastMessage(); }

private:
    struct Frame {
        int16_t data[SamplesPerFrame == 0 ? 1 : SamplesPerFrame];
        size_t samples;
    };

    static size_t storageSlots() { return Slots == 0 ? 1 : Slots; }

    static bool validFormat(const nobro_audio_format_t &format) {
        return SamplesPerFrame != 0 &&
               format.sample_rate_hz >= 8000U &&
               format.sample_rate_hz <= 192000U &&
               format.samples_per_frame != 0 &&
               format.samples_per_frame <= SamplesPerFrame &&
               (format.channels == 1U || format.channels == 2U) &&
               format.bits_per_sample == 16U;
    }

    static void increment(uint32_t &value) {
        if (value != UINT32_MAX) {
            ++value;
        }
    }

    static void add(uint32_t &value, size_t increment_by) {
        const uint32_t increment =
            increment_by > UINT32_MAX ? UINT32_MAX
                                      : static_cast<uint32_t>(increment_by);
        value = increment > UINT32_MAX - value ? UINT32_MAX
                                                : value + increment;
    }

    void clearQueue() {
        for (size_t index = 0; index < storageSlots(); ++index) {
            frames_[index].samples = 0;
        }
        read_ = 0;
        write_ = 0;
        queued_ = 0;
    }

    NiusAudioWeActEs8311Board &module_;
    Frame frames_[Slots == 0 ? 1 : Slots];
    size_t read_;
    size_t write_;
    size_t queued_;
    nobro_audio_format_t format_{};
    nobro_audio_state_t state_;
    nobro_audio_diagnostics_t diagnostics_;
};

}  // namespace nobro

#endif  // !defined(NOBRO_AUDIO_DISABLED)

#endif
