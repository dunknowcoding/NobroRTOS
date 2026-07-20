#ifndef NOBRO_ARDUINO_ESP_BLE_H
#define NOBRO_ARDUINO_ESP_BLE_H

#include "nobro_wireless.h"

#if !defined(NOBRO_ESP_BLE_DISABLED)

#if !defined(ARDUINO_ARCH_ESP32)
#error "NobroArduinoEspBLE.h requires an Arduino-ESP32 board profile"
#endif

#include <BLEAdvertising.h>
#include <BLE2902.h>
#include <BLECharacteristic.h>
#include <BLEDevice.h>
#include <BLEServer.h>

namespace nobro {

/*
 * One-instance BLE peripheral facade over the BLE library bundled with the
 * selected Arduino-ESP32 board package. Arduino-ESP32 3.3.10 selects
 * Bluedroid on classic ESP32 and NimBLE on ESP32-C3/S3. The facade does not
 * replace or hide that vendor-owned host, controller, task, callback, or heap
 * state.
 *
 * Vendor callbacks copy into one fixed event ring under the ESP32 port mux.
 * Overflow is reported to poll() as NOBRO_STACK_QUEUE_FULL; callbacks never
 * allocate Nobro-owned queue nodes or run application work.
 */
class ArduinoEspBleStack : private BLEServerCallbacks,
                           private BLECharacteristicCallbacks {
public:
    ArduinoEspBleStack()
        : server_(0),
          service_(0),
          characteristic_(0),
          descriptor_(0),
          advertising_(0),
          state_(NOBRO_STACK_DOWN),
          connected_(false),
          advertising_active_(false),
          advertising_config_failed_(false),
          queue_overflowed_(false),
          owns_global_stack_(false),
          last_call_us_(0),
          event_head_(0),
          event_tail_(0),
          event_count_(0),
          pending_events_{},
          diagnostics_{},
          mux_(portMUX_INITIALIZER_UNLOCKED) {}

    ~ArduinoEspBleStack() {
        if (owns_global_stack_) {
            resetVendorStack();
        }
    }

    ArduinoEspBleStack(const ArduinoEspBleStack &) = delete;
    ArduinoEspBleStack &operator=(const ArduinoEspBleStack &) = delete;

    nobro_stack_identity_t identity() const {
        const nobro_stack_identity_t value = {
            "arduino-esp-ble",
            NOBRO_STACK_FAMILY_BLE,
            NOBRO_BLE_GATT_VALUE_MAX,
            1,
            1,
            1,
            1,
        };
        return value;
    }

    static const char *vendorHost() {
#if defined(CONFIG_BLUEDROID_ENABLED)
        return "bluedroid";
#elif defined(CONFIG_NIMBLE_ENABLED)
        return "nimble";
#else
        return "unsupported";
#endif
    }

    nobro_stack_result_t mount() {
        if (state_ == NOBRO_STACK_QUIESCED && owns_global_stack_) {
            resetEventState();
            resetGattValue();
            state_ = NOBRO_STACK_READY;
            return NOBRO_STACK_OK;
        }
        if (state_ != NOBRO_STACK_DOWN && state_ != NOBRO_STACK_QUIESCED) {
            return NOBRO_STACK_BUSY;
        }
        if (claimed() && !owns_global_stack_) {
            return NOBRO_STACK_BUSY;
        }
        if (vendorHost()[0] == 'u') {
            return NOBRO_STACK_INVALID_IDENTITY;
        }

        claimed() = true;
        owns_global_stack_ = true;
        state_ = NOBRO_STACK_STARTING;
        resetEventState();
        const uint32_t started = micros();
        if (!BLEDevice::init(localName())) {
            last_call_us_ = static_cast<uint32_t>(micros() - started);
            return failMount();
        }
#if defined(CONFIG_BLUEDROID_ENABLED)
        activeInstance() = this;
        BLEDevice::setCustomGapHandler(bluedroidGapEvent);
#endif
        server_ = BLEDevice::createServer();
        if (server_ == 0) {
            last_call_us_ = static_cast<uint32_t>(micros() - started);
            return failMount();
        }
        server_->setCallbacks(this);
        service_ = server_->createService(serviceUuid());
        if (service_ == 0) {
            last_call_us_ = static_cast<uint32_t>(micros() - started);
            return failMount();
        }
        characteristic_ = service_->createCharacteristic(
            characteristicUuid(),
            BLECharacteristic::PROPERTY_READ |
                BLECharacteristic::PROPERTY_WRITE |
                BLECharacteristic::PROPERTY_NOTIFY);
        if (characteristic_ == 0) {
            last_call_us_ = static_cast<uint32_t>(micros() - started);
            return failMount();
        }
        characteristic_->setCallbacks(this);
#if defined(CONFIG_BLUEDROID_ENABLED)
        /*
         * The installed classic ESP32 Bluedroid host requires an explicit
         * 0x2902 descriptor. The installed NimBLE path synthesizes its CCCD
         * from the notify property and warns that callers must not add one.
         */
        descriptor_ = new BLE2902();
        if (descriptor_ == 0) {
            last_call_us_ = static_cast<uint32_t>(micros() - started);
            return failMount();
        }
        characteristic_->addDescriptor(descriptor_);
#endif
        const uint8_t initial = 0;
        characteristic_->setValue(&initial, 1);
        service_->start();
        advertising_ = BLEDevice::getAdvertising();
        if (advertising_ == 0) {
            last_call_us_ = static_cast<uint32_t>(micros() - started);
            return failMount();
        }
        advertising_->setScanResponse(true);
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        state_ = NOBRO_STACK_READY;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t advertise(const uint8_t *payload,
                                   size_t length,
                                   uint64_t now_us,
                                   uint64_t deadline_us) {
        if (state_ != NOBRO_STACK_READY || advertising_ == 0) {
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

        BLEAdvertisementData advertisement;
        advertisement.setFlags(0x06);
        advertisement.setCompleteServices(BLEUUID(serviceUuid()));
        String manufacturer(reinterpret_cast<const char *>(payload), length);
        advertisement.setManufacturerData(manufacturer);
        BLEAdvertisementData scan_response;
        scan_response.setName(localName());

        const uint32_t started = micros();
        bool advertised = false;
#if defined(CONFIG_BLUEDROID_ENABLED)
        /*
         * Bluedroid configures raw advertising and scan-response data
         * asynchronously. The bundled high-level setters return before those
         * operations complete, and start() can otherwise race their GAP
         * callbacks. Observe the package's custom GAP hook and do not report
         * success until the controller confirms advertising started.
         */
        beginAdvertisingTransition();
        const bool configured =
            advertising_->setAdvertisementData(advertisement) &&
            advertising_->setScanResponseData(scan_response);
        if (configured) {
            advertising_->start();
            const uint64_t budget = deadline_us - now_us;
            const uint32_t bounded_budget =
                budget > UINT32_MAX
                    ? UINT32_MAX
                    : static_cast<uint32_t>(budget);
            advertised = awaitAdvertising(true, bounded_budget);
        }
#elif defined(CONFIG_NIMBLE_ENABLED)
        const bool configured =
            advertising_->setAdvertisementData(advertisement) &&
            advertising_->setScanResponseData(scan_response);
        advertised = configured && advertising_->start();
#endif
        last_call_us_ = static_cast<uint32_t>(micros() - started);
        if (static_cast<uint64_t>(last_call_us_) > deadline_us - now_us) {
            advertising_->stop();
            increment(diagnostics_.deadline_misses);
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        if (!advertised) {
            return fault();
        }
        increment(diagnostics_.advertisements);
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t stopAdvertising() {
        if (state_ != NOBRO_STACK_READY || advertising_ == 0) {
            return NOBRO_STACK_NOT_READY;
        }
        const bool stopped = advertising_->stop();
#if defined(CONFIG_BLUEDROID_ENABLED)
        if ((!stopped && isAdvertisingActive()) ||
            !awaitAdvertising(false, 1000000U)) {
            return fault();
        }
#else
        if (!stopped) {
            return fault();
        }
#endif
        increment(diagnostics_.advertisement_stops);
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t poll(nobro_ble_event_t &event, bool &available) {
        available = false;
        clearEvent(event);
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }

        portENTER_CRITICAL(&mux_);
        if (queue_overflowed_) {
            queue_overflowed_ = false;
            portEXIT_CRITICAL(&mux_);
            return NOBRO_STACK_QUEUE_FULL;
        }
        if (event_count_ != 0) {
            event = pending_events_[event_head_];
            clearEvent(pending_events_[event_head_]);
            event_head_ = static_cast<uint8_t>(
                (event_head_ + 1U) % eventCapacity());
            --event_count_;
            available = true;
        }
        portEXIT_CRITICAL(&mux_);
        if (available) {
            increment(diagnostics_.events);
        }
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t respondGatt(uint16_t connection_id,
                                     uint16_t attribute_handle,
                                     const uint8_t *value,
                                     size_t length) {
        if (state_ != NOBRO_STACK_READY || characteristic_ == 0) {
            return NOBRO_STACK_NOT_READY;
        }
        if (connection_id != logicalConnection() ||
            attribute_handle != logicalCharacteristic() ||
            (length != 0 && value == 0) ||
            length > NOBRO_BLE_GATT_VALUE_MAX || !isConnected()) {
            return NOBRO_STACK_INVALID_CONFIG;
        }
        characteristic_->setValue(value, length);
        characteristic_->notify();
        increment(diagnostics_.gatt_responses);
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t disconnect() {
        if (state_ != NOBRO_STACK_READY || server_ == 0) {
            return NOBRO_STACK_NOT_READY;
        }
        if (!isConnected()) {
            return NOBRO_STACK_OK;
        }
        const uint16_t connection = server_->getConnId();
#if defined(CONFIG_BLUEDROID_ENABLED)
        server_->disconnect(connection);
#elif defined(CONFIG_NIMBLE_ENABLED)
        if (server_->disconnect(connection) != 0) {
            return fault();
        }
#else
        return NOBRO_STACK_INVALID_IDENTITY;
#endif
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t quiesce() {
        if (state_ == NOBRO_STACK_DOWN || state_ == NOBRO_STACK_QUIESCED) {
            state_ = NOBRO_STACK_QUIESCED;
            return NOBRO_STACK_OK;
        }
        if (state_ == NOBRO_STACK_READY && isConnected()) {
            const nobro_stack_result_t stopped = disconnect();
            if (stopped != NOBRO_STACK_OK) {
                return fault();
            }
        }
        /*
         * Bluedroid invokes the application disconnect callback before it
         * removes the peer and decrements BLEServer's connection count.
         * Do not expose quiescence until both the facade event and the
         * vendor server agree that the session is gone; otherwise an
         * immediate remount can race stale GATT connection bookkeeping.
         */
        if (server_ != 0 && !awaitDisconnected(1000000U)) {
            return fault();
        }
        state_ = NOBRO_STACK_STARTING;
        if (advertising_ != 0) {
            const bool stopped = advertising_->stop();
#if defined(CONFIG_BLUEDROID_ENABLED)
            if ((!stopped && isAdvertisingActive()) ||
                !awaitAdvertising(false, 1000000U)) {
                return fault();
            }
#else
            (void)stopped;
#endif
        }
        resetEventState();
        state_ = NOBRO_STACK_QUIESCED;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t recover() {
        if (state_ == NOBRO_STACK_READY && owns_global_stack_) {
            const nobro_stack_result_t stopped = quiesce();
            if (stopped != NOBRO_STACK_OK) {
                return stopped;
            }
        }
        if (state_ == NOBRO_STACK_QUIESCED && owns_global_stack_) {
            resetEventState();
            resetGattValue();
            state_ = NOBRO_STACK_READY;
            increment(diagnostics_.recoveries);
            return NOBRO_STACK_OK;
        }
        if (owns_global_stack_ && state_ != NOBRO_STACK_DOWN) {
            resetVendorStack();
        }
        state_ = NOBRO_STACK_DOWN;
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
    bool vendorManagedTasks() const { return true; }
    bool globalController() const { return true; }
    nobro_ble_stack_diagnostics_t diagnostics() const { return diagnostics_; }

private:
    static const char *serviceUuid() { return "fff0"; }
    static const char *characteristicUuid() { return "fff1"; }
    static const char *localName() { return "NobroRTOS"; }
    static uint16_t logicalConnection() { return 1; }
    static uint16_t logicalCharacteristic() { return 1; }
    static uint8_t eventCapacity() { return 4; }

    static bool &claimed() {
        static bool value = false;
        return value;
    }

#if defined(CONFIG_BLUEDROID_ENABLED)
    static ArduinoEspBleStack *&activeInstance() {
        static ArduinoEspBleStack *value = 0;
        return value;
    }

    static void bluedroidGapEvent(esp_gap_ble_cb_event_t event,
                                  esp_ble_gap_cb_param_t *param) {
        ArduinoEspBleStack *instance = activeInstance();
        if (instance != 0) {
            instance->captureBluedroidGapEvent(event, param);
        }
    }

    void captureBluedroidGapEvent(esp_gap_ble_cb_event_t event,
                                  esp_ble_gap_cb_param_t *param) {
        portENTER_CRITICAL(&mux_);
        if (event == ESP_GAP_BLE_ADV_START_COMPLETE_EVT) {
            advertising_active_ =
                param->adv_start_cmpl.status == ESP_BT_STATUS_SUCCESS;
        } else if (event == ESP_GAP_BLE_ADV_STOP_COMPLETE_EVT) {
            advertising_active_ = false;
        } else if (event == ESP_GAP_BLE_ADV_DATA_RAW_SET_COMPLETE_EVT &&
                   param->adv_data_raw_cmpl.status !=
                       ESP_BT_STATUS_SUCCESS) {
            advertising_config_failed_ = true;
        } else if (
            event == ESP_GAP_BLE_SCAN_RSP_DATA_RAW_SET_COMPLETE_EVT &&
            param->scan_rsp_data_raw_cmpl.status !=
                ESP_BT_STATUS_SUCCESS) {
            advertising_config_failed_ = true;
        }
        portEXIT_CRITICAL(&mux_);
    }
#endif

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

    void resetGattValue() {
        if (characteristic_ != 0) {
            const uint8_t initial = 0;
            characteristic_->setValue(&initial, 1);
        }
    }

    bool isConnected() {
        portENTER_CRITICAL(&mux_);
        const bool value = connected_;
        portEXIT_CRITICAL(&mux_);
        return value;
    }

    bool awaitDisconnected(uint32_t timeout_us) {
        const uint32_t started = micros();
        while (isConnected() ||
               (server_ != 0 && server_->getConnectedCount() != 0)) {
            if (static_cast<uint32_t>(micros() - started) >= timeout_us) {
                return false;
            }
            delay(1);
        }
        return true;
    }

    void beginAdvertisingTransition() {
        portENTER_CRITICAL(&mux_);
        advertising_active_ = false;
        advertising_config_failed_ = false;
        portEXIT_CRITICAL(&mux_);
    }

    bool isAdvertisingActive() {
        portENTER_CRITICAL(&mux_);
        const bool value = advertising_active_;
        portEXIT_CRITICAL(&mux_);
        return value;
    }

    bool awaitAdvertising(bool expected, uint32_t timeout_us) {
        const uint32_t started = micros();
        while (isAdvertisingActive() != expected) {
            portENTER_CRITICAL(&mux_);
            const bool config_failed = advertising_config_failed_;
            portEXIT_CRITICAL(&mux_);
            if (config_failed) {
                return false;
            }
            if (static_cast<uint32_t>(micros() - started) >= timeout_us) {
                return false;
            }
            delay(1);
        }
        return true;
    }

    void queueEvent(nobro_ble_event_kind_t kind,
                    const uint8_t *value = 0,
                    size_t length = 0) {
        portENTER_CRITICAL(&mux_);
        if (event_count_ == eventCapacity()) {
            queue_overflowed_ = true;
            portEXIT_CRITICAL(&mux_);
            return;
        }
        nobro_ble_event_t &pending = pending_events_[event_tail_];
        clearEvent(pending);
        pending.kind = kind;
        pending.connection_id = logicalConnection();
        if (kind == NOBRO_BLE_GATT_READ ||
            kind == NOBRO_BLE_GATT_WRITE ||
            kind == NOBRO_BLE_NOTIFICATION_COMPLETE) {
            pending.attribute_handle = logicalCharacteristic();
        }
        const size_t bounded =
            length < sizeof(pending.value)
                ? length
                : sizeof(pending.value);
        for (size_t index = 0; index < bounded; ++index) {
            pending.value[index] = value[index];
        }
        pending.value_len = static_cast<uint8_t>(bounded);
        event_tail_ = static_cast<uint8_t>(
            (event_tail_ + 1U) % eventCapacity());
        ++event_count_;
        portEXIT_CRITICAL(&mux_);
    }

    void captureValue(nobro_ble_event_kind_t kind,
                      BLECharacteristic *characteristic) {
        /*
         * The bundled BLEValue already owns the callback-time bytes. Reading
         * that storage directly avoids constructing another Arduino String in
         * the vendor callback (and therefore avoids an avoidable heap
         * allocation on both the Bluedroid and NimBLE paths).
         */
        const uint8_t *value = characteristic->getData();
        const size_t vendor_length = characteristic->getLength();
        const size_t length =
            vendor_length < NOBRO_BLE_GATT_VALUE_MAX
                ? vendor_length
                : NOBRO_BLE_GATT_VALUE_MAX;
        queueEvent(kind, value, length);
    }

    void onConnect(BLEServer *) override {
        portENTER_CRITICAL(&mux_);
        connected_ = true;
        portEXIT_CRITICAL(&mux_);
        queueEvent(NOBRO_BLE_CONNECTED);
    }

    void onDisconnect(BLEServer *) override {
        portENTER_CRITICAL(&mux_);
        connected_ = false;
        advertising_active_ = false;
        advertising_config_failed_ = false;
        portEXIT_CRITICAL(&mux_);
        queueEvent(NOBRO_BLE_DISCONNECTED);
    }

#if defined(CONFIG_BLUEDROID_ENABLED)
    void onRead(BLECharacteristic *characteristic,
                esp_ble_gatts_cb_param_t *) override {
        captureValue(NOBRO_BLE_GATT_READ, characteristic);
    }
    void onWrite(BLECharacteristic *characteristic,
                 esp_ble_gatts_cb_param_t *) override {
        captureValue(NOBRO_BLE_GATT_WRITE, characteristic);
    }
#endif

#if defined(CONFIG_NIMBLE_ENABLED)
    void onRead(BLECharacteristic *characteristic,
                ble_gap_conn_desc *) override {
        captureValue(NOBRO_BLE_GATT_READ, characteristic);
    }
    void onWrite(BLECharacteristic *characteristic,
                 ble_gap_conn_desc *) override {
        captureValue(NOBRO_BLE_GATT_WRITE, characteristic);
    }
#endif

    void onStatus(BLECharacteristic *,
                  BLECharacteristicCallbacks::Status status,
                  uint32_t) override {
        if (status == BLECharacteristicCallbacks::SUCCESS_NOTIFY) {
            queueEvent(NOBRO_BLE_NOTIFICATION_COMPLETE);
        }
    }

    void resetPointers() {
        server_ = 0;
        service_ = 0;
        characteristic_ = 0;
        descriptor_ = 0;
        advertising_ = 0;
    }

    void resetEventState() {
        portENTER_CRITICAL(&mux_);
        connected_ = false;
        queue_overflowed_ = false;
        event_head_ = 0;
        event_tail_ = 0;
        event_count_ = 0;
        for (uint8_t index = 0; index < eventCapacity(); ++index) {
            clearEvent(pending_events_[index]);
        }
        portEXIT_CRITICAL(&mux_);
    }

    void resetVendorStack() {
        if (advertising_ != 0) {
            advertising_->stop();
        }
        if (characteristic_ != 0) {
            characteristic_->setCallbacks(0);
        }
        if (server_ != 0) {
            server_->setCallbacks(0);
        }
#if defined(CONFIG_BLUEDROID_ENABLED)
        BLEDevice::setCustomGapHandler(0);
        if (activeInstance() == this) {
            activeInstance() = 0;
        }
#endif
        BLEDevice::deinit(false);
        resetPointers();
        resetEventState();
        owns_global_stack_ = false;
        claimed() = false;
    }

    nobro_stack_result_t failMount() {
        if (characteristic_ != 0) {
            characteristic_->setCallbacks(0);
        }
        if (server_ != 0) {
            server_->setCallbacks(0);
        }
#if defined(CONFIG_BLUEDROID_ENABLED)
        BLEDevice::setCustomGapHandler(0);
        if (activeInstance() == this) {
            activeInstance() = 0;
        }
#endif
        BLEDevice::deinit(false);
        resetPointers();
        resetEventState();
        owns_global_stack_ = false;
        claimed() = false;
        return fault();
    }

    nobro_stack_result_t fault() {
        state_ = NOBRO_STACK_FAULTED;
        increment(diagnostics_.transport_faults);
        return NOBRO_STACK_BACKEND_FAULT;
    }

    BLEServer *server_;
    BLEService *service_;
    BLECharacteristic *characteristic_;
    BLE2902 *descriptor_;
    BLEAdvertising *advertising_;
    nobro_stack_state_t state_;
    bool connected_;
    bool advertising_active_;
    bool advertising_config_failed_;
    bool queue_overflowed_;
    bool owns_global_stack_;
    uint32_t last_call_us_;
    uint8_t event_head_;
    uint8_t event_tail_;
    uint8_t event_count_;
    nobro_ble_event_t pending_events_[4];
    nobro_ble_stack_diagnostics_t diagnostics_;
    portMUX_TYPE mux_;
};

}  // namespace nobro

#endif  // !defined(NOBRO_ESP_BLE_DISABLED)

#endif
