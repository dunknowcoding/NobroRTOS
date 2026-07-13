#ifndef NOBRO_NIUS_WIRELESS_H
#define NOBRO_NIUS_WIRELESS_H

#include <NiusWireless.h>
#include "nobro_wireless.h"

namespace nobro {

class NiusWirelessHealthAdapter {
public:
    explicit NiusWirelessHealthAdapter(NiusBase &module) : module_(module), diagnostics_{} {}

    bool begin() {
        const bool ready = module_.begin();
        if (!ready) diagnostics_.read_errors++;
        return ready;
    }

    bool ready() { return module_.isReady(); }

    bool recover() {
        module_.reset();
        if (!module_.isReady()) {
            diagnostics_.recovery_failures++;
            return false;
        }
        diagnostics_.recoveries++;
        return true;
    }

    nobro_wireless_diagnostics_t diagnostics() const { return diagnostics_; }

protected:
    NiusBase &module_;
    nobro_wireless_diagnostics_t diagnostics_;
};

class NiusLoRaAdapter : public NiusWirelessHealthAdapter {
public:
    explicit NiusLoRaAdapter(NiusLoRaBase &radio)
        : NiusWirelessHealthAdapter(radio), radio_(radio) {}

    bool send(const uint8_t *payload, size_t length) {
        if (payload == nullptr || length > 255 || radio_.beginPacket() != NIUS_LORA_OK) {
            diagnostics_.tx_rejected++;
            return false;
        }
        for (size_t index = 0; index < length; ++index) {
            if (radio_.write(payload[index]) != 1) {
                diagnostics_.tx_rejected++;
                return false;
            }
        }
        if (radio_.endPacket(false) != NIUS_LORA_OK) {
            diagnostics_.tx_rejected++;
            return false;
        }
        diagnostics_.tx_accepted++;
        return true;
    }

    size_t receive(uint8_t *destination, size_t capacity) {
        const uint8_t pending = radio_.parsePacket();
        if (pending == 0) return 0;
        if (destination == nullptr || capacity < pending) {
            diagnostics_.read_errors++;
            return 0;
        }
        const uint8_t received = radio_.readBuf(destination, pending);
        if (received != 0) diagnostics_.rx_packets++;
        return received;
    }

private:
    NiusLoRaBase &radio_;
};

}  // namespace nobro

#endif
