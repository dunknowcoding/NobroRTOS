#ifndef NOBRO_ARDUINO_WIFI_S3_H
#define NOBRO_ARDUINO_WIFI_S3_H

#include "nobro_wireless.h"

#if !defined(NOBRO_WIFI_S3_DISABLED)

#if !defined(ARDUINO_UNOWIFIR4)
#error "NobroArduinoWiFiS3.h requires the Arduino UNO R4 WiFi board profile"
#endif

#include <WiFiS3.h>

namespace nobro {

/*
 * Bounded association/lifecycle facade for the Arduino Renesas WiFiS3 stack.
 *
 * This object stores no credentials and allocates no heap itself. Credentials
 * remain runtime-only borrowed values. WiFiS3 is a
 * process-wide vendor stack that uses std::string and controller-owned
 * resources internally. Its begin/scan/status calls are synchronous. A
 * deadline miss can therefore be measured and reported after a call returns,
 * but the wrapper cannot preempt the vendor call.
 *
 * The facade intentionally stops at association. TCP/UDP clients and their
 * endpoints remain separate, caller-owned data-plane objects.
 */
class ArduinoWiFiS3Stack {
public:
    ArduinoWiFiS3Stack()
        : state_(NOBRO_STACK_DOWN),
          link_state_(NOBRO_WIRELESS_DOWN),
          last_call_us_(0),
          diagnostics_{} {}

    nobro_stack_identity_t identity() const {
        const nobro_stack_identity_t value = {
            "arduino-wifis3",
            NOBRO_STACK_FAMILY_WIFI,
            1460,
            1,
            1,
            0,
            0,
        };
        return value;
    }

    nobro_stack_result_t mount() {
        if (state_ != NOBRO_STACK_DOWN && state_ != NOBRO_STACK_QUIESCED) {
            return NOBRO_STACK_BUSY;
        }
        state_ = NOBRO_STACK_STARTING;
        link_state_ = NOBRO_WIRELESS_DOWN;
        state_ = NOBRO_STACK_READY;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t scan(nobro_wifi_network_t *results,
                              size_t capacity,
                              size_t &written) {
        written = 0;
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        if (capacity != 0 && results == 0) {
            return NOBRO_STACK_INVALID_CONFIG;
        }
        const uint32_t started = micros();
        const int8_t found = WiFi.scanNetworks();
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        if (found < 0) {
            fault();
            return NOBRO_STACK_BACKEND_FAULT;
        }
        increment(diagnostics_.scans);
        const size_t available = static_cast<size_t>(found);
        written = available < capacity ? available : capacity;
        for (size_t index = 0; index < written; ++index) {
            nobro_wifi_network_t &network = results[index];
            clearNetwork(network);
            const char *ssid = WiFi.SSID(static_cast<uint8_t>(index));
            if (ssid == 0) {
                fault();
                written = index;
                return NOBRO_STACK_BACKEND_FAULT;
            }
            size_t length = 0;
            while (ssid[length] != '\0' && length < sizeof(network.ssid)) {
                network.ssid[length] = static_cast<uint8_t>(ssid[length]);
                ++length;
            }
            if (length == 0 || ssid[length] != '\0') {
                fault();
                written = index;
                return NOBRO_STACK_BACKEND_FAULT;
            }
            network.ssid_len = static_cast<uint8_t>(length);
            network.channel = WiFi.channel(static_cast<uint8_t>(index));
            const int32_t rssi = WiFi.RSSI(static_cast<uint8_t>(index));
            network.rssi_dbm =
                rssi < -128 ? -128 : (rssi > 127 ? 127 : static_cast<int8_t>(rssi));
            network.secured =
                WiFi.encryptionType(static_cast<uint8_t>(index)) != ENC_TYPE_NONE;
        }
        add(diagnostics_.scan_results, written);
        if (available > written) {
            add(diagnostics_.truncated_scan_results, available - written);
        }
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t join(const nobro_wifi_credentials_t &credentials,
                              uint64_t now_us,
                              uint64_t deadline_us) {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        if (!validCredentials(credentials)) {
            return NOBRO_STACK_INVALID_CONFIG;
        }
        if (deadline_us <= now_us) {
            increment(diagnostics_.deadline_misses);
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        const uint64_t requested_us = deadline_us - now_us;
        const uint32_t timeout_us =
            requested_us > maxCallUs() ? maxCallUs()
                                       : static_cast<uint32_t>(requested_us);
        uint32_t timeout_ms = (timeout_us + 999U) / 1000U;
        if (timeout_ms == 0) {
            timeout_ms = 1;
        }

        char ssid[33] = {};
        char secret[64] = {};
        copyText(ssid, credentials.ssid, credentials.ssid_len);
        copyText(secret, credentials.secret, credentials.secret_len);

        increment(diagnostics_.join_attempts);
        state_ = NOBRO_STACK_STARTING;
        link_state_ = NOBRO_WIRELESS_JOINING;
        WiFi.setTimeout(timeout_ms);
        const uint32_t started = micros();
        const int result = credentials.secret_len == 0
                               ? WiFi.begin(ssid)
                               : WiFi.begin(ssid, secret);
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        WiFi.setTimeout(defaultVendorTimeoutMs());

        if (static_cast<uint64_t>(last_call_us_) > requested_us) {
            WiFi.disconnect();
            link_state_ = NOBRO_WIRELESS_DOWN;
            state_ = NOBRO_STACK_READY;
            increment(diagnostics_.deadline_misses);
            increment(diagnostics_.join_failures);
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        if (result != WL_CONNECTED) {
            link_state_ = NOBRO_WIRELESS_DOWN;
            state_ = NOBRO_STACK_READY;
            increment(diagnostics_.join_failures);
            return NOBRO_STACK_ASSOCIATION_REJECTED;
        }
        link_state_ = NOBRO_WIRELESS_UP;
        state_ = NOBRO_STACK_READY;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t poll() {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        const uint32_t started = micros();
        const uint8_t status = WiFi.status();
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        if (status == WL_CONNECTED) {
            link_state_ = NOBRO_WIRELESS_UP;
        } else if (status == WL_NO_MODULE) {
            fault();
            return NOBRO_STACK_BACKEND_FAULT;
        } else {
            link_state_ = NOBRO_WIRELESS_DOWN;
        }
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t leave() {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        const uint32_t started = micros();
        const int result = WiFi.disconnect();
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        link_state_ = NOBRO_WIRELESS_DOWN;
        if (result != 1) {
            fault();
            return NOBRO_STACK_BACKEND_FAULT;
        }
        increment(diagnostics_.leaves);
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t quiesce() {
        if (state_ == NOBRO_STACK_DOWN || state_ == NOBRO_STACK_QUIESCED) {
            state_ = NOBRO_STACK_QUIESCED;
            link_state_ = NOBRO_WIRELESS_DOWN;
            return NOBRO_STACK_OK;
        }
        const uint32_t started = micros();
        WiFi.disconnect();
        WiFi.end();
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        link_state_ = NOBRO_WIRELESS_DOWN;
        state_ = NOBRO_STACK_QUIESCED;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t recover() {
        state_ = NOBRO_STACK_STARTING;
        const uint32_t started = micros();
        WiFi.end();
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        link_state_ = NOBRO_WIRELESS_DOWN;
        state_ = NOBRO_STACK_READY;
        increment(diagnostics_.recoveries);
        return NOBRO_STACK_OK;
    }

    nobro_stack_state_t state() const { return state_; }
    nobro_wireless_state_t linkState() const { return link_state_; }
    uint32_t lastCallUs() const { return last_call_us_; }
    size_t staticRamBytes() const { return sizeof(*this); }
    size_t maxScanResults() const { return WIFI_MAX_SSID_COUNT; }
    bool vendorManagedHeap() const { return true; }
    nobro_wifi_stack_diagnostics_t diagnostics() const { return diagnostics_; }

private:
    static uint32_t maxCallUs() { return 60000000UL; }
    static uint32_t defaultVendorTimeoutMs() { return 10000UL; }

    static bool validCredentials(const nobro_wifi_credentials_t &value) {
        if (value.ssid == 0 || value.ssid_len == 0 || value.ssid_len > 32 ||
            (value.secret_len != 0 &&
             (value.secret == 0 || value.secret_len < 8 ||
              value.secret_len > 63))) {
            return false;
        }
        return validAtText(value.ssid, value.ssid_len) &&
               (value.secret_len == 0 ||
                validAtText(value.secret, value.secret_len));
    }

    static bool validAtText(const uint8_t *value, size_t length) {
        for (size_t index = 0; index < length; ++index) {
            if (value[index] < 32 || value[index] > 126 ||
                value[index] == static_cast<uint8_t>(',')) {
                return false;
            }
        }
        return true;
    }

    static void copyText(char *destination, const uint8_t *source, size_t length) {
        for (size_t index = 0; index < length; ++index) {
            destination[index] = static_cast<char>(source[index]);
        }
        destination[length] = '\0';
    }

    static void clearNetwork(nobro_wifi_network_t &network) {
        for (size_t index = 0; index < sizeof(network.ssid); ++index) {
            network.ssid[index] = 0;
        }
        network.ssid_len = 0;
        network.channel = 0;
        network.rssi_dbm = 0;
        network.secured = false;
    }

    static void increment(uint32_t &value) {
        if (value != UINT32_MAX) {
            ++value;
        }
    }

    static void add(uint32_t &value, size_t amount) {
        const uint32_t increment =
            amount > UINT32_MAX ? UINT32_MAX : static_cast<uint32_t>(amount);
        value = increment > UINT32_MAX - value ? UINT32_MAX : value + increment;
    }

    void fault() {
        link_state_ = NOBRO_WIRELESS_DOWN;
        state_ = NOBRO_STACK_FAULTED;
        increment(diagnostics_.transport_faults);
    }

    nobro_stack_state_t state_;
    nobro_wireless_state_t link_state_;
    uint32_t last_call_us_;
    nobro_wifi_stack_diagnostics_t diagnostics_;
};

}  // namespace nobro

#endif  // !defined(NOBRO_WIFI_S3_DISABLED)

#endif
