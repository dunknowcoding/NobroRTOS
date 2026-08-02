#ifndef NOBRO_NIUS_ZIGBEE_H
#define NOBRO_NIUS_ZIGBEE_H

#include <CC2530Radio.h>
#include "nobro_wireless.h"

namespace nobro {

struct NiusZigbeeDiagnostics {
    nobro_wireless_diagnostics_t link;
    uint32_t deadline_misses;
};

class NiusCc2530Adapter {
public:
    explicit NiusCc2530Adapter(CC2530Radio &radio)
        : radio_(radio), channel_(0), ready_(false), suspended_(false),
          diagnostics_{} {}

    bool begin(uint8_t channel = 11, uint32_t baud = 115200) {
        if (channel < 11 || channel > 26 || ready_) return false;
        if (!radio_.begin(channel, baud)) {
            diagnostics_.link.read_errors++;
            return false;
        }
        channel_ = channel;
        ready_ = true;
        suspended_ = false;
        return true;
    }

    bool sendRaw(const uint8_t *payload, size_t bytes, uint8_t retries = 0) {
        if (!usable() || payload == nullptr || bytes == 0 ||
            bytes > CC2530Radio::kMaxPayload) {
            diagnostics_.link.tx_rejected++;
            return false;
        }
        const bool sent = retries == 0
                              ? radio_.send(payload, static_cast<uint8_t>(bytes))
                              : radio_.sendWithRetries(payload, static_cast<uint8_t>(bytes),
                                                      retries);
        if (!sent) {
            diagnostics_.link.tx_rejected++;
            return false;
        }
        diagnostics_.link.tx_accepted++;
        return true;
    }

    bool sendAps(uint16_t pan_id, uint16_t destination, uint16_t source,
                 uint8_t destination_endpoint, uint16_t cluster_id,
                 uint16_t profile_id, uint8_t source_endpoint,
                 const uint8_t *payload, size_t bytes, bool ack_request = true) {
        if (!usable() || payload == nullptr || bytes > UINT8_MAX) {
            diagnostics_.link.tx_rejected++;
            return false;
        }
        if (!radio_.sendApsData(pan_id, destination, source, destination, source,
                                destination_endpoint, cluster_id, profile_id,
                                source_endpoint, payload, static_cast<uint8_t>(bytes),
                                ZigbeeNwk::kDefaultRadius, ack_request)) {
            diagnostics_.link.tx_rejected++;
            return false;
        }
        diagnostics_.link.tx_accepted++;
        return true;
    }

    bool process(uint32_t deadline_us) {
        if (!usable()) return false;
        radio_.poll();
        if (static_cast<int32_t>(micros() - deadline_us) > 0) {
            diagnostics_.deadline_misses++;
            return false;
        }
        return true;
    }

    void quiesce() { suspended_ = true; }

    bool recover() {
        if (!ready_ || !suspended_ || !radio_.ping() ||
            !radio_.setChannel(channel_)) {
            diagnostics_.link.recovery_failures++;
            return false;
        }
        suspended_ = false;
        diagnostics_.link.recoveries++;
        return true;
    }

    void release() {
        ready_ = false;
        suspended_ = false;
        channel_ = 0;
    }

    bool macStats(CC2530MacStats &stats) {
        if (!usable() || !radio_.getMacStats(stats)) {
            diagnostics_.link.read_errors++;
            return false;
        }
        return true;
    }

    bool usable() const { return ready_ && !suspended_; }
    NiusZigbeeDiagnostics diagnostics() const { return diagnostics_; }

private:
    CC2530Radio &radio_;
    uint8_t channel_;
    bool ready_;
    bool suspended_;
    NiusZigbeeDiagnostics diagnostics_;
};

}  // namespace nobro

#endif
