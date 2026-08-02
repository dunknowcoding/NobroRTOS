#ifndef NOBRO_NIUS_WIRELESS_H
#define NOBRO_NIUS_WIRELESS_H

#include <NiusWireless.h>
#include "nobro_wireless.h"

namespace nobro {

class NiusWirelessHealthAdapter {
public:
    explicit NiusWirelessHealthAdapter(NiusBase &module)
        : module_(module), suspended_(false), diagnostics_{} {}

    bool begin() {
        const bool ready = module_.begin();
        if (!ready) diagnostics_.read_errors++;
        suspended_ = false;
        return ready;
    }

    bool ready() { return !suspended_ && module_.isReady(); }

    // Fail-closed software suspend: lifecycle parity with the audio adapter's
    // quiesce() and the IMU adapter's suspend(). A quiesced link reports
    // not-ready and rejects tx/rx until recover() clears it. The concrete radio
    // exposes no vendor "end" call, so this is an honest software gate, not a
    // claimed hardware power-down.
    void quiesce() { suspended_ = true; }
    bool quiesced() const { return suspended_; }

    bool recover() {
        module_.reset();
        if (!module_.isReady()) {
            diagnostics_.recovery_failures++;
            return false;
        }
        suspended_ = false;
        diagnostics_.recoveries++;
        return true;
    }

    nobro_wireless_diagnostics_t diagnostics() const { return diagnostics_; }

protected:
    NiusBase &module_;
    bool suspended_;
    nobro_wireless_diagnostics_t diagnostics_;
};

class NiusLoRaAdapter : public NiusWirelessHealthAdapter {
public:
    explicit NiusLoRaAdapter(NiusLoRaBase &radio)
        : NiusWirelessHealthAdapter(radio), radio_(radio) {}

    bool send(const uint8_t *payload, size_t length) {
        if (!ready()) {
            diagnostics_.tx_rejected++;
            return false;
        }
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
        if (!ready()) {
            diagnostics_.read_errors++;
            return 0;
        }
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

    bool quiesce() {
        if (radio_.sleep() != NIUS_LORA_OK) {
            diagnostics_.read_errors++;
            return false;
        }
        suspended_ = true;
        return true;
    }

    bool recover() {
        if (radio_.standby() != NIUS_LORA_OK || !radio_.isReady()) {
            diagnostics_.recovery_failures++;
            return false;
        }
        suspended_ = false;
        diagnostics_.recoveries++;
        return true;
    }

private:
    NiusLoRaBase &radio_;
};

class NiusNrf24Adapter : public NiusWirelessHealthAdapter {
public:
    explicit NiusNrf24Adapter(NiusNRF24L01 &radio)
        : NiusWirelessHealthAdapter(radio), radio_(radio), listening_(false),
          resume_listening_(false) {}

    void listen() {
        if (!ready()) return;
        radio_.startListening();
        listening_ = true;
    }

    void stopListening() {
        radio_.stopListening();
        listening_ = false;
    }

    bool send(const uint8_t *payload, size_t length) {
        if (!ready() || payload == nullptr || length == 0 || length > 32) {
            diagnostics_.tx_rejected++;
            return false;
        }
        if (!radio_.writeRadio(payload, static_cast<uint8_t>(length))) {
            diagnostics_.tx_rejected++;
            return false;
        }
        diagnostics_.tx_accepted++;
        return true;
    }

    size_t receive(uint8_t *destination, size_t capacity) {
        if (!ready() || destination == nullptr || capacity == 0) {
            diagnostics_.read_errors++;
            return 0;
        }
        if (!radio_.available()) return 0;
        uint8_t received = 0;
        const uint8_t bounded_capacity = capacity > 32 ? 32 : static_cast<uint8_t>(capacity);
        if (!radio_.readRadio(destination, bounded_capacity, received)) {
            diagnostics_.read_errors++;
            return 0;
        }
        diagnostics_.rx_packets++;
        return received;
    }

    bool quiesce() {
        resume_listening_ = listening_;
        if (!radio_.powerDown()) {
            diagnostics_.read_errors++;
            return false;
        }
        listening_ = false;
        suspended_ = true;
        return true;
    }

    bool recover() {
        if (!radio_.powerUp() || !radio_.isReady()) {
            diagnostics_.recovery_failures++;
            return false;
        }
        suspended_ = false;
        if (resume_listening_) {
            radio_.startListening();
            listening_ = true;
        }
        diagnostics_.recoveries++;
        return true;
    }

private:
    NiusNRF24L01 &radio_;
    bool listening_;
    bool resume_listening_;
};

class NiusRc522Adapter : public NiusWirelessHealthAdapter {
public:
    explicit NiusRc522Adapter(NiusRC522 &reader)
        : NiusWirelessHealthAdapter(reader), reader_(reader) {}

    size_t pollUid(uint8_t *destination, size_t capacity, bool irq_wake = false) {
        if (!ready() || destination == nullptr || capacity == 0) {
            diagnostics_.read_errors++;
            return 0;
        }
        if (!(irq_wake ? reader_.cardPresentWake() : reader_.cardPresent())) return 0;
        uint8_t uid[10] = {0};
        uint8_t length = sizeof(uid);
        if (!reader_.getUIDBytes(uid, length) || length > capacity) {
            diagnostics_.read_errors++;
            reader_.halt();
            return 0;
        }
        for (uint8_t index = 0; index < length; ++index) destination[index] = uid[index];
        reader_.halt();
        diagnostics_.rx_packets++;
        return length;
    }

    void quiesce() {
        reader_.antennaOff();
        suspended_ = true;
    }

    bool recover() {
        reader_.antennaOn();
        reader_.reset();
        if (!reader_.isReady()) {
            diagnostics_.recovery_failures++;
            return false;
        }
        suspended_ = false;
        diagnostics_.recoveries++;
        return true;
    }

private:
    NiusRC522 &reader_;
};

class NiusPn532Adapter : public NiusWirelessHealthAdapter {
public:
    explicit NiusPn532Adapter(NiusPN532 &reader)
        : NiusWirelessHealthAdapter(reader), reader_(reader) {}

    size_t pollUid(uint8_t *destination, size_t capacity, bool irq_wake = false) {
        if (!ready() || destination == nullptr || capacity == 0) {
            diagnostics_.read_errors++;
            return 0;
        }
        if (!(irq_wake ? reader_.cardPresentWake() : reader_.cardPresent())) return 0;
        uint8_t uid[10] = {0};
        uint8_t length = sizeof(uid);
        if (!reader_.getUIDBytes(uid, length) || length > capacity) {
            diagnostics_.read_errors++;
            reader_.halt();
            return 0;
        }
        for (uint8_t index = 0; index < length; ++index) destination[index] = uid[index];
        reader_.halt();
        diagnostics_.rx_packets++;
        return length;
    }

    bool quiesce() {
        if (!reader_.setRFField(1, 0)) {
            diagnostics_.read_errors++;
            return false;
        }
        suspended_ = true;
        return true;
    }

    bool recover() {
        reader_.reset();
        if (!reader_.begin() || !reader_.setRFField(1, 1)) {
            diagnostics_.recovery_failures++;
            return false;
        }
        suspended_ = false;
        diagnostics_.recoveries++;
        return true;
    }

private:
    NiusPN532 &reader_;
};

template <typename SerialRadio>
class NiusSerialLinkAdapter : public NiusWirelessHealthAdapter {
public:
    explicit NiusSerialLinkAdapter(SerialRadio &radio)
        : NiusWirelessHealthAdapter(radio), radio_(radio) {}

    bool send(const uint8_t *payload, size_t length) {
        if (!ready() || (payload == nullptr && length != 0)) {
            diagnostics_.tx_rejected++;
            return false;
        }
        const size_t written = radio_.write(payload, length);
        if (written != length) {
            diagnostics_.tx_rejected++;
            return false;
        }
        diagnostics_.tx_accepted++;
        return true;
    }

    size_t receive(uint8_t *destination, size_t capacity, uint32_t timeout_ms = 0) {
        if (!ready() || (destination == nullptr && capacity != 0)) {
            diagnostics_.read_errors++;
            return 0;
        }
        const size_t received = radio_.read(destination, capacity, timeout_ms);
        if (received != 0) diagnostics_.rx_packets++;
        return received;
    }

    bool recover() {
        if (!radio_.begin()) {
            diagnostics_.recovery_failures++;
            return false;
        }
        suspended_ = false;
        diagnostics_.recoveries++;
        return true;
    }

private:
    SerialRadio &radio_;
};

}  // namespace nobro

#endif
