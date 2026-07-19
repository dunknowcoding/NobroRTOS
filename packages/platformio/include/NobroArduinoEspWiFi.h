#ifndef NOBRO_ARDUINO_ESP_WIFI_H
#define NOBRO_ARDUINO_ESP_WIFI_H

#include "nobro_wireless.h"

#if !defined(NOBRO_ESP_WIFI_DISABLED)

#if !defined(ARDUINO_ARCH_ESP32)
#error "NobroArduinoEspWiFi.h requires an Arduino-ESP32 board profile"
#endif

#include <WiFi.h>
#include <esp_wifi.h>

namespace nobro {

/*
 * Bounded station-association facade for the Arduino-ESP32 WiFi stack.
 *
 * Credentials are runtime-only: borrowed for join(), copied into fixed stack
 * buffers, and never retained by this object. persistent(false) prevents the facade
 * from asking Arduino-ESP32 to save credentials in NVS. ESP-IDF still owns
 * process-wide tasks, callbacks, heap, radio, and TCP/IP resources; those must
 * be measured for each exact board composition before admission pricing.
 *
 * The facade stops at association. TCP/UDP/TLS clients remain separate,
 * caller-owned data-plane objects with their own endpoint and buffer policy.
 */
class ArduinoEspWiFiStack {
public:
    ArduinoEspWiFiStack()
        : state_(NOBRO_STACK_DOWN),
          link_state_(NOBRO_WIRELESS_DOWN),
          last_call_us_(0),
          diagnostics_{} {}

    nobro_stack_identity_t identity() const {
        const nobro_stack_identity_t value = {
            "arduino-esp-wifi",
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
        WiFi.persistent(false);
        /*
         * Arduino-ESP32 mode(WIFI_STA) accepts the interface mode before the
         * station netif has necessarily emitted ESP_NETIF_STARTED_BIT.
         * STA.begin(false) performs the board-core-owned bounded readiness
         * wait. Skipping it can make an immediate esp_wifi_scan_start fail
         * even though mode() returned true.
         */
        if (!WiFi.mode(WIFI_STA) || !WiFi.STA.begin(false)) {
            fault();
            return NOBRO_STACK_BACKEND_FAULT;
        }
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

        /*
         * Arduino-ESP32 3.3.10 starts its asynchronous scan before it sets
         * WIFI_SCANNING_BIT. A fast WIFI_EVENT_SCAN_DONE can therefore be
         * discarded by WiFiScanClass::_scanDone(). Use the ESP-IDF driver
         * bundled with that same board package in blocking mode instead.
         * Fetching one record at a time keeps Nobro's workspace fixed and
         * avoids Arduino String/calloc result storage.
         */
        wifi_scan_config_t config = {};
        config.show_hidden = false;
        config.scan_type = WIFI_SCAN_TYPE_ACTIVE;
        config.scan_time.active.min = 100;
        config.scan_time.active.max = 300;

        esp_wifi_clear_ap_list();
        const uint32_t started = micros();
        const esp_err_t scan_result = esp_wifi_scan_start(&config, true);
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        if (scan_result != ESP_OK) {
            esp_wifi_clear_ap_list();
            fault();
            return NOBRO_STACK_BACKEND_FAULT;
        }

        uint16_t found = 0;
        if (esp_wifi_scan_get_ap_num(&found) != ESP_OK) {
            esp_wifi_clear_ap_list();
            fault();
            return NOBRO_STACK_BACKEND_FAULT;
        }
        increment(diagnostics_.scans);
        const size_t available = static_cast<size_t>(found);
        written = available < capacity ? available : capacity;
        for (size_t index = 0; index < written; ++index) {
            wifi_ap_record_t record = {};
            if (esp_wifi_scan_get_ap_record(&record) != ESP_OK) {
                esp_wifi_clear_ap_list();
                fault();
                written = index;
                return NOBRO_STACK_BACKEND_FAULT;
            }
            nobro_wifi_network_t &network = results[index];
            clearNetwork(network);
            size_t length = 0;
            while (length < sizeof(record.ssid) && record.ssid[length] != 0) {
                ++length;
            }
            if (length == 0 || length > sizeof(network.ssid)) {
                esp_wifi_clear_ap_list();
                fault();
                written = index;
                return NOBRO_STACK_BACKEND_FAULT;
            }
            for (size_t byte = 0; byte < length; ++byte) {
                network.ssid[byte] = record.ssid[byte];
            }
            network.ssid_len = static_cast<uint8_t>(length);
            network.channel = record.primary;
            const int32_t rssi = record.rssi;
            network.rssi_dbm =
                rssi < -128 ? -128 : (rssi > 127 ? 127 : static_cast<int8_t>(rssi));
            network.secured = record.authmode != WIFI_AUTH_OPEN;
        }
        add(diagnostics_.scan_results, written);
        if (available > written) {
            add(diagnostics_.truncated_scan_results, available - written);
        }
        esp_wifi_clear_ap_list();
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

        const uint64_t requested = deadline_us - now_us;
        const uint32_t timeout_us =
            requested > maxCallUs() ? maxCallUs() : static_cast<uint32_t>(requested);
        char ssid[33] = {};
        char secret[64] = {};
        copyText(ssid, credentials.ssid, credentials.ssid_len);
        copyText(secret, credentials.secret, credentials.secret_len);

        increment(diagnostics_.join_attempts);
        state_ = NOBRO_STACK_STARTING;
        link_state_ = NOBRO_WIRELESS_JOINING;
        WiFi.persistent(false);
        const uint32_t started = micros();
        wl_status_t status =
            credentials.secret_len == 0 ? WiFi.begin(ssid) : WiFi.begin(ssid, secret);
        while (status != WL_CONNECTED &&
               static_cast<uint32_t>(micros() - started) < timeout_us) {
            if (status == WL_CONNECT_FAILED || status == WL_NO_SSID_AVAIL) {
                break;
            }
            delay(1);
            status = WiFi.status();
        }
        last_call_us_ = static_cast<uint32_t>(micros() - started);

        if (status == WL_CONNECTED) {
            link_state_ = NOBRO_WIRELESS_UP;
            state_ = NOBRO_STACK_READY;
            return NOBRO_STACK_OK;
        }
        if (!clearFailedAssociation()) {
            fault();
            return NOBRO_STACK_BACKEND_FAULT;
        }
        link_state_ = NOBRO_WIRELESS_DOWN;
        state_ = NOBRO_STACK_READY;
        increment(diagnostics_.join_failures);
        if (last_call_us_ >= timeout_us) {
            increment(diagnostics_.deadline_misses);
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        return NOBRO_STACK_ASSOCIATION_REJECTED;
    }

    nobro_stack_result_t poll() {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        const uint32_t started = micros();
        const wl_status_t status = WiFi.status();
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        if (status == WL_CONNECTED) {
            link_state_ = NOBRO_WIRELESS_UP;
        } else if (status == WL_NO_SHIELD) {
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
        const bool ok = WiFi.disconnect(false, true);
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        link_state_ = NOBRO_WIRELESS_DOWN;
        if (!ok) {
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
        const bool ok = WiFi.disconnect(true, true);
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        link_state_ = NOBRO_WIRELESS_DOWN;
        state_ = ok ? NOBRO_STACK_QUIESCED : NOBRO_STACK_FAULTED;
        if (!ok) {
            increment(diagnostics_.transport_faults);
            return NOBRO_STACK_BACKEND_FAULT;
        }
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t recover() {
        state_ = NOBRO_STACK_STARTING;
        const uint32_t started = micros();
        WiFi.disconnect(true, true);
        WiFi.persistent(false);
        const bool ok =
            WiFi.mode(WIFI_STA) && WiFi.STA.begin(false);
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        link_state_ = NOBRO_WIRELESS_DOWN;
        state_ = ok ? NOBRO_STACK_READY : NOBRO_STACK_FAULTED;
        if (!ok) {
            increment(diagnostics_.transport_faults);
            return NOBRO_STACK_BACKEND_FAULT;
        }
        increment(diagnostics_.recoveries);
        return NOBRO_STACK_OK;
    }

    nobro_stack_state_t state() const { return state_; }
    nobro_wireless_state_t linkState() const { return link_state_; }
    uint32_t lastCallUs() const { return last_call_us_; }
    size_t staticRamBytes() const { return sizeof(*this); }
    bool vendorManagedHeap() const { return true; }
    bool vendorManagedTasks() const { return true; }
    nobro_wifi_stack_diagnostics_t diagnostics() const { return diagnostics_; }

private:
    static uint32_t maxCallUs() { return 60000000UL; }

    static bool clearFailedAssociation() {
        const esp_err_t disconnected = esp_wifi_disconnect();
        if (disconnected != ESP_OK &&
            disconnected != ESP_ERR_WIFI_NOT_CONNECT) {
            return false;
        }

        /*
         * Arduino-ESP32 STA.disconnect(eraseap=true) can call
         * esp_wifi_set_config() before an in-progress association has
         * delivered its disconnect event. Retry only that transient
         * ESP_ERR_WIFI_STATE case for a bounded interval. The board package
         * is already configured for RAM-only WiFi storage by persistent(false).
         */
        wifi_config_t empty = {};
        const uint32_t started = micros();
        do {
            const esp_err_t cleared =
                esp_wifi_set_config(WIFI_IF_STA, &empty);
            if (cleared == ESP_OK) {
                return true;
            }
            if (cleared != ESP_ERR_WIFI_STATE) {
                return false;
            }
            delay(1);
        } while (static_cast<uint32_t>(micros() - started) <
                 failedCleanupUs());
        return false;
    }

    static uint32_t failedCleanupUs() { return 250000UL; }

    static bool validCredentials(const nobro_wifi_credentials_t &value) {
        if (value.ssid == 0 || value.ssid_len == 0 || value.ssid_len > 32 ||
            (value.secret_len != 0 &&
             (value.secret == 0 || value.secret_len < 8 ||
              value.secret_len > 63))) {
            return false;
        }
        return validText(value.ssid, value.ssid_len) &&
               (value.secret_len == 0 || validText(value.secret, value.secret_len));
    }

    static bool validText(const uint8_t *value, size_t length) {
        for (size_t index = 0; index < length; ++index) {
            if (value[index] < 32 || value[index] > 126) {
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

#endif  // !defined(NOBRO_ESP_WIFI_DISABLED)

#endif
