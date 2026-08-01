#ifndef NOBRO_ARDUINO_ESP8266_WIFI_H
#define NOBRO_ARDUINO_ESP8266_WIFI_H

#include "nobro_wireless.h"

#if !defined(NOBRO_ESP8266_WIFI_DISABLED)

#if !defined(ARDUINO_ARCH_ESP8266)
#error "NobroArduinoEsp8266WiFi.h requires the Arduino-ESP8266 board package"
#endif

#include <ESP8266WiFi.h>
#include <WiFiUdp.h>
#include "NobroArduinoNativeNetwork.h"

namespace nobro {

struct ArduinoEsp8266NativeProvider {
    typedef WiFiClient TcpClient;
    typedef WiFiUDP UdpSocket;

    static const char *backendId() { return "arduino-esp8266-native-ip"; }
    static uint16_t mtu() { return 1460; }
    static uint32_t rxBufferBytes() { return 0; }
    static uint32_t txBufferBytes() { return 0; }
    static bool ready() { return WiFi.status() == WL_CONNECTED; }
    static int resolve(const char *host, IPAddress &address) {
        return WiFi.hostByName(host, address);
    }
};

typedef ArduinoNativeNetwork<ArduinoEsp8266NativeProvider, 1, 1>
    ArduinoEsp8266Network;

/* Nonblocking station lifecycle for the single-core ESP8266. The facade owns
 * no credentials or heap. The Arduino core/lwIP radio stack remains
 * process-wide and heap-managed. beginJoin() accepts an association attempt;
 * poll() advances it and enforces the supplied relative deadline. */
class ArduinoEsp8266WiFiStack {
public:
    ArduinoEsp8266WiFiStack()
        : state_(NOBRO_STACK_DOWN), link_state_(NOBRO_WIRELESS_DOWN),
          join_deadline_us_(0), joining_(false), scanning_(false),
          last_call_us_(0), diagnostics_{} {}

    nobro_stack_identity_t identity() const {
        const nobro_stack_identity_t value = {
            "arduino-esp8266-wifi", NOBRO_STACK_FAMILY_WIFI,
            1460, 1, 1, 0, 0,
        };
        return value;
    }

    nobro_stack_result_t mount() {
        if (state_ != NOBRO_STACK_DOWN && state_ != NOBRO_STACK_QUIESCED)
            return NOBRO_STACK_BUSY;
        state_ = NOBRO_STACK_STARTING;
        WiFi.persistent(false);
        if (!WiFi.mode(WIFI_STA)) return fault();
        WiFi.setAutoReconnect(false);
        WiFi.disconnect(false, false);
        WiFi.scanDelete();
        joining_ = false;
        scanning_ = false;
        link_state_ = NOBRO_WIRELESS_DOWN;
        state_ = NOBRO_STACK_READY;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t beginScan() {
        if (state_ != NOBRO_STACK_READY) return NOBRO_STACK_NOT_READY;
        if (joining_ || scanning_) return NOBRO_STACK_BUSY;
        const uint32_t started = micros();
        const int8_t result = WiFi.scanNetworks(true, false);
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        if (result != WIFI_SCAN_RUNNING) return fault();
        scanning_ = true;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t pollScan(nobro_wifi_network_t *results,
                                  size_t capacity, size_t &written) {
        written = 0;
        if (state_ != NOBRO_STACK_READY) return NOBRO_STACK_NOT_READY;
        if (!scanning_) return NOBRO_STACK_NOT_READY;
        if (capacity != 0 && results == 0) return NOBRO_STACK_INVALID_CONFIG;
        const int8_t found = WiFi.scanComplete();
        if (found == WIFI_SCAN_RUNNING) return NOBRO_STACK_BUSY;
        if (found < 0) {
            scanning_ = false;
            WiFi.scanDelete();
            return fault();
        }
        increment(diagnostics_.scans);
        const size_t available = static_cast<size_t>(found);
        written = available < capacity ? available : capacity;
        for (size_t index = 0; index < written; ++index) {
            nobro_wifi_network_t &network = results[index];
            clearNetwork(network);
            const String &ssid = WiFi.SSID(static_cast<uint8_t>(index));
            const size_t length = ssid.length();
            if (length == 0 || length > sizeof(network.ssid)) {
                scanning_ = false;
                WiFi.scanDelete();
                return fault();
            }
            for (size_t byte = 0; byte < length; ++byte)
                network.ssid[byte] = static_cast<uint8_t>(ssid[byte]);
            network.ssid_len = static_cast<uint8_t>(length);
            network.channel = WiFi.channel(static_cast<uint8_t>(index));
            const int32_t rssi = WiFi.RSSI(static_cast<uint8_t>(index));
            network.rssi_dbm = rssi < -128 ? -128 :
                               (rssi > 127 ? 127 : static_cast<int8_t>(rssi));
            network.secured = WiFi.encryptionType(static_cast<uint8_t>(index)) != ENC_TYPE_NONE;
        }
        add(diagnostics_.scan_results, written);
        if (available > written) add(diagnostics_.truncated_scan_results, available - written);
        scanning_ = false;
        WiFi.scanDelete();
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t beginJoin(const nobro_wifi_credentials_t &credentials,
                                   uint64_t now_us, uint64_t deadline_us) {
        if (state_ != NOBRO_STACK_READY) return NOBRO_STACK_NOT_READY;
        if (joining_ || scanning_) return NOBRO_STACK_BUSY;
        if (!validCredentials(credentials)) return NOBRO_STACK_INVALID_CONFIG;
        if (deadline_us <= now_us) return deadlineMiss();
        const uint64_t requested = deadline_us - now_us;
        if (requested > 0x7ffffffful) return NOBRO_STACK_INVALID_CONFIG;
        char ssid[33] = {};
        char secret[64] = {};
        copyText(ssid, credentials.ssid, credentials.ssid_len);
        copyText(secret, credentials.secret, credentials.secret_len);
        WiFi.persistent(false);
        WiFi.disconnect(false, false);
        increment(diagnostics_.join_attempts);
        link_state_ = NOBRO_WIRELESS_JOINING;
        const uint32_t started = micros();
        const wl_status_t initial = credentials.secret_len == 0
                                        ? WiFi.begin(ssid)
                                        : WiFi.begin(ssid, secret);
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        if (initial == WL_CONNECT_FAILED || initial == WL_WRONG_PASSWORD) {
            link_state_ = NOBRO_WIRELESS_DOWN;
            increment(diagnostics_.join_failures);
            return NOBRO_STACK_ASSOCIATION_REJECTED;
        }
        join_deadline_us_ = started + static_cast<uint32_t>(requested);
        joining_ = true;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t poll() {
        if (state_ != NOBRO_STACK_READY) return NOBRO_STACK_NOT_READY;
        const uint32_t started = micros();
        const wl_status_t status = WiFi.status();
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        if (status == WL_CONNECTED) {
            joining_ = false;
            link_state_ = NOBRO_WIRELESS_UP;
            return NOBRO_STACK_OK;
        }
        if (!joining_) {
            link_state_ = NOBRO_WIRELESS_DOWN;
            return NOBRO_STACK_OK;
        }
        if (status == WL_CONNECT_FAILED || status == WL_NO_SSID_AVAIL ||
            status == WL_WRONG_PASSWORD) {
            joining_ = false;
            link_state_ = NOBRO_WIRELESS_DOWN;
            increment(diagnostics_.join_failures);
            return NOBRO_STACK_ASSOCIATION_REJECTED;
        }
        if (reached(micros(), join_deadline_us_)) {
            WiFi.disconnect(false, false);
            joining_ = false;
            link_state_ = NOBRO_WIRELESS_DOWN;
            increment(diagnostics_.join_failures);
            return deadlineMiss();
        }
        yield();
        return NOBRO_STACK_BUSY;
    }

    nobro_stack_result_t leave() {
        if (state_ != NOBRO_STACK_READY) return NOBRO_STACK_NOT_READY;
        const uint32_t started = micros();
        WiFi.disconnect(false, false);
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        joining_ = false;
        link_state_ = NOBRO_WIRELESS_DOWN;
        increment(diagnostics_.leaves);
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t quiesce() {
        WiFi.scanDelete();
        WiFi.disconnect(false, false);
        if (!WiFi.mode(WIFI_OFF)) return fault();
        joining_ = false;
        scanning_ = false;
        link_state_ = NOBRO_WIRELESS_DOWN;
        state_ = NOBRO_STACK_QUIESCED;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t recover() {
        state_ = NOBRO_STACK_QUIESCED;
        const nobro_stack_result_t result = mount();
        if (result == NOBRO_STACK_OK) increment(diagnostics_.recoveries);
        return result;
    }

    nobro_stack_state_t state() const { return state_; }
    nobro_wireless_state_t linkState() const { return link_state_; }
    uint32_t lastCallUs() const { return last_call_us_; }
    bool joining() const { return joining_; }
    bool scanning() const { return scanning_; }
    bool vendorManagedHeap() const { return true; }
    size_t staticRamBytes() const { return sizeof(*this); }
    nobro_wifi_stack_diagnostics_t diagnostics() const { return diagnostics_; }

private:
    static bool reached(uint32_t now, uint32_t deadline) {
        return static_cast<int32_t>(now - deadline) >= 0;
    }
    static bool validCredentials(const nobro_wifi_credentials_t &value) {
        if (value.ssid == 0 || value.ssid_len == 0 || value.ssid_len > 32 ||
            (value.secret_len != 0 &&
             (value.secret == 0 || value.secret_len < 8 || value.secret_len > 63)))
            return false;
        return validText(value.ssid, value.ssid_len) &&
               (value.secret_len == 0 || validText(value.secret, value.secret_len));
    }
    static bool validText(const uint8_t *value, size_t length) {
        for (size_t index = 0; index < length; ++index) {
            if (value[index] < 32 || value[index] > 126) return false;
        }
        return true;
    }
    static void copyText(char *destination, const uint8_t *source, size_t length) {
        for (size_t index = 0; index < length; ++index)
            destination[index] = static_cast<char>(source[index]);
        destination[length] = '\0';
    }
    static void clearNetwork(nobro_wifi_network_t &network) {
        for (size_t index = 0; index < sizeof(network.ssid); ++index) network.ssid[index] = 0;
        network.ssid_len = 0;
        network.channel = 0;
        network.rssi_dbm = 0;
        network.secured = false;
    }
    static void increment(uint32_t &value) { if (value != UINT32_MAX) ++value; }
    static void add(uint32_t &value, size_t amount) {
        const uint32_t delta = amount > UINT32_MAX ? UINT32_MAX : static_cast<uint32_t>(amount);
        value = delta > UINT32_MAX - value ? UINT32_MAX : value + delta;
    }
    nobro_stack_result_t deadlineMiss() {
        increment(diagnostics_.deadline_misses);
        return NOBRO_STACK_DEADLINE_ELAPSED;
    }
    nobro_stack_result_t fault() {
        joining_ = false;
        scanning_ = false;
        link_state_ = NOBRO_WIRELESS_DOWN;
        state_ = NOBRO_STACK_FAULTED;
        increment(diagnostics_.transport_faults);
        return NOBRO_STACK_BACKEND_FAULT;
    }

    nobro_stack_state_t state_;
    nobro_wireless_state_t link_state_;
    uint32_t join_deadline_us_;
    bool joining_;
    bool scanning_;
    uint32_t last_call_us_;
    nobro_wifi_stack_diagnostics_t diagnostics_;
};

}  // namespace nobro

#endif
#endif
