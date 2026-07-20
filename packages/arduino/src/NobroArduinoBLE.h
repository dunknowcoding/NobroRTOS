#ifndef NOBRO_ARDUINO_BLE_H
#define NOBRO_ARDUINO_BLE_H

#include "nobro_wireless.h"

#if !defined(NOBRO_ARDUINO_BLE_DISABLED)

#if !defined(ARDUINO_UNOR4_WIFI)
#error "NobroArduinoBLE.h currently requires the Arduino UNO R4 WiFi board profile"
#endif

#include <ArduinoBLE.h>
#include <WiFiS3.h>
#include <local/BLELocalCharacteristic.h>

namespace nobro {

namespace detail {

/*
 * ArduinoBLE 2.1.0 clears a retained service characteristic without releasing
 * that retain. Probe the local reference count after BLE.end() and compensate
 * only when that exact retain is still present. Keeping this check here makes
 * repeated service registration bounded while remaining safe if upstream
 * later releases the retain itself.
 */
class ManagedBleCharacteristic : public BLECharacteristic {
public:
    ManagedBleCharacteristic(const char *uuid,
                             uint16_t permissions,
                             int value_size,
                             bool fixed_length)
        : BLECharacteristic(uuid, permissions, value_size, fixed_length) {}

    bool releaseClearedServiceRetain() {
        BLELocalCharacteristic *attribute = local();
        if (attribute == 0) {
            return false;
        }
        attribute->retain();
        const int references = attribute->release();
        if (references == 1) {
            return true;
        }
        return references == 2 && attribute->release() == 1;
    }
};

}  // namespace detail

/*
 * One-service/one-characteristic ArduinoBLE peripheral facade.
 *
 * The exact UNO R4 path is ArduinoBLE's official HCIVirtualTransportAT over
 * the WiFiS3 ModemClass supplied by the installed Arduino Renesas board
 * package. ArduinoBLE owns global HCI/GATT state and dynamic allocations, so
 * only one mounted facade is admitted. Target compilation does not establish
 * physical GATT behavior or simultaneous WiFi/BLE operation.
 */
class ArduinoBleStack {
public:
    ArduinoBleStack()
        : service_(serviceUuid()),
          characteristic_(characteristicUuid(), BLERead | BLEWrite | BLENotify,
                          NOBRO_BLE_GATT_VALUE_MAX, false),
          state_(NOBRO_STACK_DOWN),
          connected_(false),
          owns_global_stack_(false),
          service_registered_(false),
          controller_started_(false),
          last_call_us_(0),
          diagnostics_{} {}

    ~ArduinoBleStack() {
        if (owns_global_stack_) {
            if (endStack()) {
                claimed() = false;
            }
        }
    }

    ArduinoBleStack(const ArduinoBleStack &) = delete;
    ArduinoBleStack &operator=(const ArduinoBleStack &) = delete;

    nobro_stack_identity_t identity() const {
        const nobro_stack_identity_t value = {
            "arduino-ble",
            NOBRO_STACK_FAMILY_BLE,
            NOBRO_BLE_GATT_VALUE_MAX,
            1,
            1,
            1,
            1,
        };
        return value;
    }

    nobro_stack_result_t mount() {
        if (state_ != NOBRO_STACK_DOWN && state_ != NOBRO_STACK_QUIESCED) {
            return NOBRO_STACK_BUSY;
        }
        if (claimed() && !owns_global_stack_) {
            return NOBRO_STACK_BUSY;
        }
        claimed() = true;
        owns_global_stack_ = true;
        state_ = NOBRO_STACK_STARTING;
        const uint32_t started = micros();
        const int begun = BLE.begin();
        controller_started_ = begun != 0;
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        if (!begun || !BLE.setLocalName(localName()) ||
            !BLE.setAdvertisedService(service_)) {
            return failMount();
        }
        service_.addCharacteristic(characteristic_);
        BLE.addService(service_);
        service_registered_ = true;
        const uint8_t initial = 0;
        if (!characteristic_.writeValue(&initial, 1)) {
            return failMount();
        }
        connected_ = false;
        state_ = NOBRO_STACK_READY;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t advertise(const uint8_t *payload,
                                   size_t length,
                                   uint64_t now_us,
                                   uint64_t deadline_us) {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        if ((length != 0 && payload == 0) ||
            length > NOBRO_BLE_GATT_VALUE_MAX) {
            return NOBRO_STACK_INVALID_CONFIG;
        }
        if (deadline_us <= now_us) {
            increment(diagnostics_.deadline_misses);
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        if (!BLE.setManufacturerData(0xffff, payload,
                                     static_cast<int>(length))) {
            return fault();
        }
        const uint32_t started = micros();
        const int result = BLE.advertise();
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        if (static_cast<uint64_t>(last_call_us_) > deadline_us - now_us) {
            BLE.stopAdvertise();
            increment(diagnostics_.deadline_misses);
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        if (!result) {
            return fault();
        }
        increment(diagnostics_.advertisements);
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t stopAdvertising() {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        BLE.stopAdvertise();
        increment(diagnostics_.advertisement_stops);
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t poll(nobro_ble_event_t &event, bool &available) {
        available = false;
        clearEvent(event);
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        BLE.poll();
        const bool now_connected = BLE.connected();
        if (now_connected != connected_) {
            connected_ = now_connected;
            event.kind = now_connected ? NOBRO_BLE_CONNECTED
                                       : NOBRO_BLE_DISCONNECTED;
            event.connection_id = logicalConnection();
            available = true;
        } else if (characteristic_.written()) {
            const int length = characteristic_.valueLength();
            if (length < 0 ||
                length > static_cast<int>(NOBRO_BLE_GATT_VALUE_MAX)) {
                return fault();
            }
            event.kind = NOBRO_BLE_GATT_WRITE;
            event.connection_id = logicalConnection();
            event.attribute_handle = logicalCharacteristic();
            event.value_len = static_cast<uint8_t>(length);
            for (int index = 0; index < length; ++index) {
                event.value[index] = characteristic_.value()[index];
            }
            available = true;
        }
        if (available) {
            increment(diagnostics_.events);
        }
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t respondGatt(uint16_t connection_id,
                                     uint16_t attribute_handle,
                                     const uint8_t *value,
                                     size_t length) {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        if (!connected_ || connection_id != logicalConnection() ||
            attribute_handle != logicalCharacteristic() ||
            (length != 0 && value == 0) ||
            length > NOBRO_BLE_GATT_VALUE_MAX) {
            return NOBRO_STACK_INVALID_CONFIG;
        }
        if (!characteristic_.writeValue(value, static_cast<int>(length))) {
            return fault();
        }
        increment(diagnostics_.gatt_responses);
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t disconnect() {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        BLE.poll();
        if (!BLE.connected()) {
            connected_ = false;
            return NOBRO_STACK_OK;
        }
        if (!BLE.disconnect()) {
            return fault();
        }
        connected_ = false;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t quiesce() {
        if (state_ == NOBRO_STACK_DOWN || state_ == NOBRO_STACK_QUIESCED) {
            state_ = NOBRO_STACK_QUIESCED;
            return NOBRO_STACK_OK;
        }
        if (!endStack()) {
            return fault();
        }
        connected_ = false;
        state_ = NOBRO_STACK_QUIESCED;
        owns_global_stack_ = false;
        claimed() = false;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t recover() {
        if (!owns_global_stack_) {
            return mount();
        }
        if (!endStack()) {
            return fault();
        }
        connected_ = false;
        state_ = NOBRO_STACK_DOWN;
        owns_global_stack_ = false;
        claimed() = false;
        const nobro_stack_result_t result = mount();
        if (result == NOBRO_STACK_OK) {
            increment(diagnostics_.recoveries);
        }
        return result;
    }

    nobro_stack_state_t state() const { return state_; }
    uint32_t lastCallUs() const { return last_call_us_; }
    size_t staticRamBytes() const { return sizeof(*this); }
    bool vendorManagedHeap() const { return true; }
    bool globalController() const { return true; }
    nobro_ble_stack_diagnostics_t diagnostics() const { return diagnostics_; }

private:
    static const char *serviceUuid() {
        return "1cce1000-7a35-4d8f-a05a-287d3b773201";
    }
    static const char *characteristicUuid() {
        return "1cce1001-7a35-4d8f-a05a-287d3b773201";
    }
    static const char *localName() { return "NobroRTOS"; }
    static uint16_t logicalConnection() { return 1; }
    static uint16_t logicalCharacteristic() { return 1; }

    static bool &claimed() {
        static bool value = false;
        return value;
    }

    static void clearEvent(nobro_ble_event_t &event) {
        event.kind = NOBRO_BLE_DISCONNECTED;
        event.connection_id = 0;
        event.attribute_handle = 0;
        event.value_len = 0;
        for (size_t index = 0; index < sizeof(event.value); ++index) {
            event.value[index] = 0;
        }
    }

    static void increment(uint32_t &value) {
        if (value != UINT32_MAX) {
            ++value;
        }
    }

    bool endStack() {
        BLE.stopAdvertise();
        if (BLE.connected()) {
            BLE.disconnect();
        }
        BLE.end();

        bool clean = true;
        if (service_registered_) {
            clean = characteristic_.releaseClearedServiceRetain();
            service_registered_ = false;
        }

        /*
         * ArduinoBLE 2.1.0's UNO R4 HCIVirtualTransportAT::end() is empty even
         * though the official 0.6.0 bridge implements AT+HCIEND. Without this
         * bounded teardown, a later BLE.begin() reinitializes an already-live
         * controller and cannot provide deterministic quiesce/recovery.
         */
        if (controller_started_) {
            std::string response;
            clean =
                modem.write(std::string(PROMPT(_HCI_END)), response,
                            CMD(_HCI_END)) &&
                clean;
            controller_started_ = false;
        }
        return clean;
    }

    nobro_stack_result_t failMount() {
        connected_ = false;
        if (!endStack()) {
            return fault();
        }
        owns_global_stack_ = false;
        claimed() = false;
        return fault();
    }

    nobro_stack_result_t fault() {
        state_ = NOBRO_STACK_FAULTED;
        increment(diagnostics_.transport_faults);
        return NOBRO_STACK_BACKEND_FAULT;
    }

    BLEService service_;
    detail::ManagedBleCharacteristic characteristic_;
    nobro_stack_state_t state_;
    bool connected_;
    bool owns_global_stack_;
    bool service_registered_;
    bool controller_started_;
    uint32_t last_call_us_;
    nobro_ble_stack_diagnostics_t diagnostics_;
};

}  // namespace nobro

#endif  // !defined(NOBRO_ARDUINO_BLE_DISABLED)

#endif
