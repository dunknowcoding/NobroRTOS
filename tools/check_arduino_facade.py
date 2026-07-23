#!/usr/bin/env python3
"""Compile and execute allocation-free NobroApp positive/negative contracts."""
import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
HEADER = ROOT / "packages" / "arduino" / "src"
PROVIDER_EXAMPLE = ROOT / "packages" / "arduino" / "examples" / "ProviderApp"
SOURCE = r'''
#include <cassert>
#include <cstring>
#include "NobroRTOS.h"
int main() {
  nobro::NobroApp<3, 1> ok;
  nobro::TaskId motor = ok.task("motor", nobro::hz(200), nobro::CONTROL);
  nobro::TaskId imu = ok.task("imu", nobro::hz(100));
  ok.wire(imu, motor, 8).budget(motor, 800);
  assert(ok.admit() && ok.taskCount() == 2 && ok.wireCount() == 1);

  nobro::NobroApp<1, 1> capacity;
  capacity.task("a", 1000);
  assert(!capacity.task("b", 1000).valid());
  assert(!capacity.admit() && capacity.error() == nobro::APP_TASK_CAPACITY);
  assert(std::strcmp(capacity.errorCode(), "NOBRO-E053") == 0);
  assert(std::strcmp(capacity.errorText(), "Application task capacity is exceeded.") == 0);

  nobro::NobroApp<2, 1> duplicate;
  duplicate.task("same", 1000);
  assert(!duplicate.task("same", 2000).valid());
  assert(duplicate.error() == nobro::APP_DUPLICATE_TASK);
  assert(std::strcmp(duplicate.errorCode(), "NOBRO-E056") == 0);

  nobro::NobroApp<1, 1> invalid_name;
  assert(!invalid_name.task("BadName", 1000).valid());
  assert(invalid_name.error() == nobro::APP_INVALID_NAME);
  assert(std::strcmp(invalid_name.errorCode(), "NOBRO-E051") == 0);

  nobro::NobroApp<1, 1> deadline;
  nobro::TaskId task = deadline.task("a", 1000, nobro::CONTROL);
  deadline.budget(task, 1001);
  assert(!deadline.admit() && deadline.error() == nobro::APP_BUDGET_EXCEEDS_PERIOD);

  nobro::NobroApp<1, 1> memory(13000, 3300);
  memory.task("large", 10000, nobro::SERVICE);
  assert(!memory.admit() && memory.error() == nobro::APP_RESOURCE_BUDGET);

  nobro::NobroApp<1, 1> zero_execution_budget;
  nobro::TaskId zero_task = zero_execution_budget.task("zero", 10000, nobro::SERVICE);
  zero_execution_budget.budget(zero_task, 0);
  assert(!zero_execution_budget.admit());
  assert(zero_execution_budget.error() == nobro::APP_INVALID_BUDGET);
  zero_execution_budget.budget(zero_task, 1);
  assert(zero_execution_budget.error() == nobro::APP_INVALID_BUDGET);

  nobro::NobroApp<1, 1> zero_board_budget(0, 0xFFFFFFFFul);
  assert(!zero_board_budget.admit());
  assert(zero_board_budget.error() == nobro::APP_RESOURCE_BUDGET);

  nobro::NobroApp<1, 1> zero_task_memory;
  nobro::TaskId zero_memory_task =
      zero_task_memory.task("zero-memory", 10000, nobro::SERVICE);
  zero_task_memory.memory(zero_memory_task, 0, 1);
  assert(!zero_task_memory.admit());
  assert(zero_task_memory.error() == nobro::APP_RESOURCE_BUDGET);

  nobro::NobroApp<1, 1> overflow(0xFFFFFFFFul, 0xFFFFFFFFul);
  nobro::TaskId overflow_task = overflow.task("overflow", 10000, nobro::SERVICE);
  const uint32_t flash_before = overflow.flashUsed();
  const uint32_t ram_before = overflow.ramUsed();
  overflow.memory(overflow_task, 0xFFFFFFFFul, 0xFFFFFFFFul);
  assert(!overflow.admit() && overflow.error() == nobro::APP_RESOURCE_BUDGET);
  assert(overflow.flashUsed() == flash_before && overflow.ramUsed() == ram_before);
  overflow.budget(overflow_task, 0);
  assert(overflow.error() == nobro::APP_RESOURCE_BUDGET);
}
'''

WIRELESS_POLICY_SOURCE = r'''
#include <cassert>
#include "NobroAdaptiveWireless.h"

static int send_calls = 0;
static nobro_wireless_send_result_t send_once_rejected(
    const uint8_t *, size_t, uint64_t, void *) {
  ++send_calls;
  return send_calls == 1 ? NOBRO_WIRELESS_SEND_BACKEND_REJECTED
                         : NOBRO_WIRELESS_SEND_OK;
}

static nobro_wireless_send_result_t always_rejected(
    const uint8_t *, size_t, uint64_t, void *) {
  return NOBRO_WIRELESS_SEND_BACKEND_REJECTED;
}

struct SendCapture {
  uint8_t first_bytes[4];
  size_t count;
};

static nobro_wireless_send_result_t capture_send(
    const uint8_t *payload, size_t length, uint64_t, void *context) {
  SendCapture *capture = static_cast<SendCapture *>(context);
  if (capture->count < 4 && length != 0) {
    capture->first_bytes[capture->count++] = payload[0];
  }
  return NOBRO_WIRELESS_SEND_OK;
}

int main() {
  nobro::WirelessPolicy responsive = nobro::WirelessPolicy::responsive(8, 64);
  assert(responsive.valid());
  assert(responsive.native().storage_mode == NOBRO_WIRELESS_STORAGE_FIXED);
  assert(responsive.native().retry.max_attempts == 3);

  nobro::WirelessPolicy low_energy =
      nobro::WirelessPolicy::lowEnergy(16, 128, 50000).callerPool();
  assert(low_energy.valid());
  assert(low_energy.native().storage_mode == NOBRO_WIRELESS_STORAGE_CALLER_POOL);
  assert(low_energy.native().batch_window_us == 50000);

  nobro::WirelessMessage message =
      nobro::WirelessMessage::bestEffort(10, 100).deadline(50).priority(7);
  assert(message.valid());
  assert(message.native().batchable);
  assert(message.native().priority == 7);
  assert(nobro::WirelessMessage::urgent(10, 20).valid());

  nobro::WirelessPolicy queue_policy = nobro::WirelessPolicy::responsive(2, 8);
  nobro::AdaptiveWirelessQueue<2, 8> queue(queue_policy);
  assert(queue.reservedBytes() >= 16);
  const uint8_t payload[] = {1, 2, 3};
  nobro::WirelessTicket ticket = queue.enqueue(
      payload, sizeof(payload), nobro::WirelessMessage::urgent(0, 100000));
  assert(ticket.valid && queue.size() == 1);
  nobro::WirelessEvent first = queue.service(0, send_once_rejected);
  assert(first.kind == nobro::WIRELESS_RETRY_AT && first.atUs == 1000);
  nobro::WirelessEvent second = queue.service(1000, send_once_rejected);
  assert(second.kind == nobro::WIRELESS_DELIVERED && second.id == ticket.id);
  assert(queue.diagnostics().retry_attempts == 1);
  assert(queue.diagnostics().delivered_messages == 1);
  nobro::WirelessTicket relative =
      queue.enqueueUrgentWithin(payload, sizeof(payload), UINT64_MAX - 5, 20);
  assert(relative.valid);
  assert(queue.service(UINT64_MAX, send_once_rejected).kind ==
         nobro::WIRELESS_DELIVERED);

  nobro::AdaptiveWirelessQueue<2, 8> ordered(queue_policy);
  const uint8_t low[] = {1};
  const uint8_t high[] = {9};
  nobro::WirelessTicket low_ticket = ordered.enqueue(
      low, sizeof(low),
      nobro::WirelessMessage::bestEffort(0, 100).priority(1).batchable(false));
  nobro::WirelessTicket high_ticket = ordered.enqueue(
      high, sizeof(high),
      nobro::WirelessMessage::bestEffort(0, 100).priority(9).batchable(false));
  assert(low_ticket.valid && high_ticket.valid);
  assert(!ordered.enqueue(low, sizeof(low),
      nobro::WirelessMessage::bestEffort(0, 100)).valid);
  SendCapture capture = {{0}, 0};
  assert(ordered.service(0, capture_send, &capture).id == high_ticket.id);
  assert(capture.count == 1 && capture.first_bytes[0] == 9);
  assert(ordered.cancel(low_ticket));
  assert(ordered.diagnostics().backpressure_rejections == 1);

  nobro::AdaptiveWirelessQueue<2, 8> deadline_order(queue_policy);
  nobro::WirelessTicket later = deadline_order.enqueue(
      low, sizeof(low), nobro::WirelessMessage::bestEffort(0, 100)
          .deadline(90).batchable(false));
  nobro::WirelessTicket sooner = deadline_order.enqueue(
      high, sizeof(high), nobro::WirelessMessage::bestEffort(10, 100)
          .deadline(20).batchable(false));
  assert(later.valid && sooner.valid);
  assert(deadline_order.service(10, capture_send, &capture).id == sooner.id);
  assert(deadline_order.service(10, capture_send, &capture).id == later.id);

  nobro::AdaptiveWirelessQueue<2, 8> batch(
      nobro::WirelessPolicy::lowEnergy(2, 8, 50));
  nobro::WirelessTicket expired = batch.enqueue(
      low, sizeof(low), nobro::WirelessMessage::bestEffort(0, 10));
  assert(expired.valid);
  assert(batch.service(11, capture_send, &capture).kind == nobro::WIRELESS_EXPIRED);
  batch.enqueue(high, sizeof(high), nobro::WirelessMessage::urgent(20, 100));
  batch.enqueue(low, sizeof(low), nobro::WirelessMessage::urgent(20, 100));
  assert(batch.serviceBatch(20, capture_send, &capture) == 2);

  nobro::AdaptiveWirelessQueue<2, 8> exhausted(
      nobro::WirelessPolicy::responsive(2, 8));
  exhausted.enqueue(high, sizeof(high),
      nobro::WirelessMessage::urgent(0, 10000));
  exhausted.enqueue(low, sizeof(low),
      nobro::WirelessMessage::urgent(0, 10000));
  assert(exhausted.serviceBatch(0, always_rejected) == 0);
  assert(exhausted.size() == 2);
  assert(exhausted.serviceBatch(1000, always_rejected) == 0);
  assert(exhausted.serviceBatch(3000, always_rejected) == 0);
  assert(exhausted.size() == 1);

  nobro::AdaptiveWirelessQueue<2, 8> invalid(
      nobro::WirelessPolicy::responsive(1, 8));
  assert(!invalid.valid());
  assert(invalid.serviceBatch(0, capture_send, &capture) == 0);
  assert(invalid.diagnostics().radio_wake_batches == 0);

  nobro::WirelessRecovery recovery =
      nobro::WirelessRecovery::exponential(3, 10, 40);
  assert(recovery.valid() && recovery.ready(0));
  assert(recovery.failed(100));
  assert(recovery.failedAttempts() == 1 && recovery.nextAttemptUs() == 110);
  assert(!recovery.ready(109) && recovery.ready(110));
  assert(recovery.failed(110) && recovery.nextAttemptUs() == 130);
  assert(!recovery.failed(130));
  recovery.reset();
  assert(recovery.failedAttempts() == 0 && recovery.ready(0));
}
'''

WIRELESS_C_SOURCE = r'''
#include <assert.h>
#include "nobro_wireless.h"
int main(void) {
  nobro_wireless_adaptive_policy_t policy =
      nobro_wireless_low_energy_policy(8u, 64u, 50000u);
  assert(nobro_wireless_policy_valid(&policy));
  policy.storage_mode = NOBRO_WIRELESS_STORAGE_HEAP;
  assert(policy.storage_mode == NOBRO_WIRELESS_STORAGE_HEAP);
  policy.storage_mode = (nobro_wireless_storage_mode_t)99;
  assert(!nobro_wireless_policy_valid(&policy));
  nobro_wireless_message_contract_t message =
      nobro_wireless_best_effort_message(10u, 100u);
  message.deadline_us = 50u;
  assert(nobro_wireless_message_valid(&message));
  message.deadline_us = 101u;
  assert(!nobro_wireless_message_valid(&message));
  nobro_wireless_recovery_state_t recovery;
  nobro_wireless_recovery_reset(&recovery);
  assert(nobro_wireless_recovery_ready(&recovery, 0u));
  assert(nobro_wireless_recovery_failed(&recovery, &policy.retry, 100u));
  assert(recovery.failed_attempts == 1u);
  assert(recovery.next_attempt_us == 10100u);
  assert(!nobro_wireless_recovery_ready(&recovery, 1000u));
  assert(nobro_wireless_recovery_ready(&recovery, 10100u));
  nobro_wireless_retry_policy_t invalid_retry = {2u, 0u, 1u};
  assert(!nobro_wireless_retry_policy_valid(&invalid_retry));
  nobro_wireless_recovery_reset(&recovery);
  assert(!nobro_wireless_recovery_failed(
      &recovery, &invalid_retry, 0u));
  assert(recovery.failed_attempts == 0u);
  return 0;
}
'''

ARDUINO_STUB = r'''
#pragma once
#include <cstddef>
#include <cstdint>
using std::size_t;
#define INPUT 0
#define OUTPUT 1
#define LOW 0
#define HIGH 1
#define A0 14
struct ArduinoStubState {
  int pin_mode_calls = 0;
  uint8_t last_pin_mode_pin = 0;
  uint8_t last_pin_mode = 0;
  int analog_read_calls = 0;
  int next_analog_read = 0;
  int analog_read_resolution_calls = 0;
  uint8_t last_analog_read_resolution = 0;
  int analog_write_resolution_calls = 0;
  uint8_t last_analog_write_resolution_pin = 0;
  uint8_t last_analog_write_resolution = 0;
  int analog_write_calls = 0;
  uint8_t last_analog_write_pin = 0;
  uint16_t last_analog_write_value = 0;
  int digital_write_calls = 0;
  int digital_write_low_calls = 0;
  int digital_write_high_calls = 0;
  uint8_t last_digital_write_pin = 0;
  uint8_t last_digital_write_value = 0;
};
static ArduinoStubState ArduinoStub;
inline uint32_t micros() { return 0; }
inline void pinMode(uint8_t pin, uint8_t mode) {
  ++ArduinoStub.pin_mode_calls;
  ArduinoStub.last_pin_mode_pin = pin;
  ArduinoStub.last_pin_mode = mode;
}
inline int analogRead(uint8_t) {
  ++ArduinoStub.analog_read_calls;
  return ArduinoStub.next_analog_read;
}
inline void analogReadResolution(uint8_t bits) {
  ++ArduinoStub.analog_read_resolution_calls;
  ArduinoStub.last_analog_read_resolution = bits;
}
inline void analogWriteResolution(uint8_t bits) {
  ++ArduinoStub.analog_write_resolution_calls;
  ArduinoStub.last_analog_write_resolution = bits;
}
inline void analogWriteResolution(uint8_t pin, uint8_t bits) {
  ++ArduinoStub.analog_write_resolution_calls;
  ArduinoStub.last_analog_write_resolution_pin = pin;
  ArduinoStub.last_analog_write_resolution = bits;
}
inline void analogWrite(uint8_t pin, uint16_t value) {
  ++ArduinoStub.analog_write_calls;
  ArduinoStub.last_analog_write_pin = pin;
  ArduinoStub.last_analog_write_value = value;
}
inline void digitalWrite(uint8_t pin, uint8_t value) {
  ++ArduinoStub.digital_write_calls;
  if (value == LOW) ++ArduinoStub.digital_write_low_calls;
  if (value == HIGH) ++ArduinoStub.digital_write_high_calls;
  ArduinoStub.last_digital_write_pin = pin;
  ArduinoStub.last_digital_write_value = value;
}
class Stream {
public:
  virtual ~Stream() {}
  virtual int available() { return 0; }
  virtual int read() { return -1; }
  virtual size_t write(const uint8_t *, size_t) { return 0; }
  virtual void flush() {}
};
'''

WIRE_STUB = r'''
#pragma once
#include "Arduino.h"
class TwoWire {
public:
  int begin_calls = 0;
  int begin_transmission_calls = 0;
  int end_transmission_calls = 0;
  size_t next_write = 0;
  uint8_t next_end_status = 0;
  bool last_stop = false;
  size_t next_request = 0;
  int read_values[8] = {};
  size_t read_count = 0;
  size_t read_index = 0;

  void begin() { ++begin_calls; }
  void setClock(uint32_t) {}
  void beginTransmission(uint8_t) { ++begin_transmission_calls; }
  size_t write(const uint8_t *, size_t) { return next_write; }
  uint8_t endTransmission(bool stop) {
    ++end_transmission_calls;
    last_stop = stop;
    return next_end_status;
  }
  size_t requestFrom(uint8_t, uint8_t, uint8_t) {
    read_index = 0;
    return next_request;
  }
  int available() { return read_index < read_count; }
  int read() { return read_index < read_count ? read_values[read_index++] : -1; }
};
static TwoWire Wire;
'''

SPI_STUB = r'''
#pragma once
#include "Arduino.h"
#define MSBFIRST 1
#define SPI_MODE0 0
class SPISettings {
public:
  SPISettings(uint32_t, uint8_t, uint8_t) {}
};
class SPIClass {
public:
  int begin_calls = 0;
  int begin_transaction_calls = 0;
  int transfer_calls = 0;
  int end_transaction_calls = 0;
  uint8_t transfer_xor = 0;

  void begin() { ++begin_calls; }
  void beginTransaction(const SPISettings &) { ++begin_transaction_calls; }
  uint8_t transfer(uint8_t value) {
    ++transfer_calls;
    return static_cast<uint8_t>(value ^ transfer_xor);
  }
  void endTransaction() { ++end_transaction_calls; }
};
static SPIClass SPI;
'''

PROVIDER_SOURCE = r'''
#include <cassert>
#define NOBRO_ARDUINO_ENABLE_I2C
#define NOBRO_ARDUINO_ENABLE_SPI
#include "NobroArduinoProviders.h"
#include "ProviderReport.h"

class FakeStream : public Stream {
public:
  size_t write_results[4] = {};
  size_t write_result_count = 0;
  size_t write_index = 0;
  size_t write_calls = 0;
  size_t last_write_length = 0;
  int read_values[4] = {};
  size_t read_count = 0;
  size_t read_index = 0;

  size_t write(const uint8_t *, size_t length) override {
    ++write_calls;
    last_write_length = length;
    return write_index < write_result_count ? write_results[write_index++] : 0;
  }
  int available() override { return read_index < read_count; }
  int read() override { return read_index < read_count ? read_values[read_index++] : -1; }
};

class CapturingShortStream : public Stream {
public:
  size_t write_limits[8] = {};
  size_t write_limit_count = 0;
  size_t write_index = 0;
  uint8_t captured[256] = {};
  size_t captured_count = 0;

  size_t write(const uint8_t *bytes, size_t length) override {
    if (write_index >= write_limit_count) return 0;
    const size_t limit = write_limits[write_index++];
    const size_t accepted = limit < length ? limit : length;
    assert(captured_count + accepted <= sizeof(captured));
    for (size_t i = 0; i < accepted; ++i) captured[captured_count++] = bytes[i];
    return accepted;
  }
};

static void test_adc_pwm_lifecycle_and_resolution() {
  ArduinoStub = ArduinoStubState();
  uint16_t sample = 77;
  nobro::ArduinoAdc adc(2, 10);
  assert(!adc.begun());
  assert(!adc.read(sample) && sample == 77);
  assert(adc.read() == 0 && ArduinoStub.analog_read_calls == 0);
  assert(adc.begin() && adc.begun());
  assert(ArduinoStub.pin_mode_calls == 1 && ArduinoStub.last_pin_mode_pin == 2);
  assert(ArduinoStub.last_pin_mode == INPUT && adc.maxSample() == 1023);
#if defined(ARDUINO_ARCH_ESP32) || defined(ARDUINO_ARCH_RENESAS) || \
    defined(ARDUINO_ARCH_NRF52)
  assert(ArduinoStub.analog_read_resolution_calls == 1);
  assert(ArduinoStub.last_analog_read_resolution == 10);
#else
  assert(ArduinoStub.analog_read_resolution_calls == 0);
#endif

  ArduinoStub.next_analog_read = 1023;
  assert(adc.read(sample) && sample == 1023);
  ArduinoStub.next_analog_read = 1024;
  assert(!adc.read(sample) && sample == 1023);
  ArduinoStub.next_analog_read = -1;
  assert(!adc.read(sample) && sample == 1023);

  nobro::ArduinoAdc zero_bit_adc(3, 0);
  const int pin_modes_before_invalid_adc = ArduinoStub.pin_mode_calls;
  assert(!zero_bit_adc.begin() && !zero_bit_adc.begun());
  assert(ArduinoStub.pin_mode_calls == pin_modes_before_invalid_adc);

  nobro::ArduinoPwm pwm(4, 8);
  assert(!pwm.begun());
  assert(!pwm.setDuty(1) && ArduinoStub.analog_write_calls == 0);
  assert(pwm.begin() && pwm.begun());
  assert(ArduinoStub.last_pin_mode_pin == 4 && ArduinoStub.last_pin_mode == OUTPUT);
  assert(ArduinoStub.analog_write_calls == 1);
  assert(ArduinoStub.last_analog_write_pin == 4 && ArduinoStub.last_analog_write_value == 0);
  assert(pwm.maxDuty() == 255 && pwm.setDuty(255));
  assert(ArduinoStub.analog_write_calls == 2 && ArduinoStub.last_analog_write_value == 255);
  assert(!pwm.setDuty(256) && ArduinoStub.analog_write_calls == 2);

  nobro::ArduinoPwm zero_bit_pwm(5, 0);
  const int pin_modes_before_invalid_pwm = ArduinoStub.pin_mode_calls;
  assert(!zero_bit_pwm.begin() && !zero_bit_pwm.begun());
  assert(ArduinoStub.pin_mode_calls == pin_modes_before_invalid_pwm);

#if defined(ARDUINO_ARCH_AVR)
  assert(nobro::ArduinoAdc::supportsResolution(10));
  assert(!nobro::ArduinoAdc::supportsResolution(12));
  assert(nobro::ArduinoPwm::supportsResolution(8));
  assert(!nobro::ArduinoPwm::supportsResolution(10));
  nobro::ArduinoAdc rejected_adc(6, 12);
  nobro::ArduinoPwm rejected_pwm(7, 10);
  assert(!rejected_adc.begin() && !rejected_pwm.begin());
#elif defined(ARDUINO_ARCH_ESP32)
  assert(nobro::ArduinoAdc::supportsResolution(16));
  assert(!nobro::ArduinoAdc::supportsResolution(17));
  assert(nobro::ArduinoPwm::supportsResolution(14));
  assert(!nobro::ArduinoPwm::supportsResolution(15));
  nobro::ArduinoAdc wide_adc(6, 16);
  nobro::ArduinoPwm wide_pwm(7, 14);
  assert(wide_adc.begin() && wide_pwm.begin());
#elif defined(ARDUINO_ARCH_RENESAS)
  assert(nobro::ArduinoAdc::supportsResolution(14));
  assert(!nobro::ArduinoAdc::supportsResolution(9));
  assert(nobro::ArduinoPwm::supportsResolution(16));
  nobro::ArduinoAdc wide_adc(6, 14);
  nobro::ArduinoPwm wide_pwm(7, 16);
  assert(wide_adc.begin() && wide_pwm.begin());
#elif defined(ARDUINO_ARCH_NRF52)
  assert(nobro::ArduinoAdc::supportsResolution(14));
  assert(!nobro::ArduinoAdc::supportsResolution(15));
  assert(nobro::ArduinoPwm::supportsResolution(16));
  nobro::ArduinoAdc wide_adc(6, 14);
  nobro::ArduinoPwm wide_pwm(7, 16);
  assert(wide_adc.begin() && wide_pwm.begin());
#else
  assert(nobro::ArduinoAdc::supportsResolution(10));
  assert(!nobro::ArduinoAdc::supportsResolution(12));
  assert(nobro::ArduinoPwm::supportsResolution(8));
  assert(!nobro::ArduinoPwm::supportsResolution(10));
  nobro::ArduinoAdc rejected_adc(6, 12);
  nobro::ArduinoPwm rejected_pwm(7, 10);
  assert(!rejected_adc.begin() && !rejected_pwm.begin());
#endif
}

static void test_resolution_isolation() {
#if defined(ARDUINO_ARCH_ESP32) || defined(ARDUINO_ARCH_RENESAS) || \
    defined(ARDUINO_ARCH_NRF52)
  ArduinoStub = ArduinoStubState();
#if defined(ARDUINO_ARCH_RENESAS)
  const uint8_t narrow_adc_bits = 10;
#else
  const uint8_t narrow_adc_bits = 8;
#endif
  const uint8_t wide_adc_bits =
#if defined(ARDUINO_ARCH_ESP32)
      12;
#else
      14;
#endif

  nobro::ArduinoAdc narrow_adc(2, narrow_adc_bits);
  nobro::ArduinoAdc wide_adc(3, wide_adc_bits);
  assert(narrow_adc.begin() && wide_adc.begin());
  assert(ArduinoStub.last_analog_read_resolution == wide_adc_bits);
  ArduinoStub.next_analog_read = 1;
  uint16_t sample = 0;
  int resolution_calls = ArduinoStub.analog_read_resolution_calls;
  assert(narrow_adc.read(sample) && sample == 1);
  assert(ArduinoStub.analog_read_resolution_calls == resolution_calls + 1);
  assert(ArduinoStub.last_analog_read_resolution == narrow_adc_bits);
  resolution_calls = ArduinoStub.analog_read_resolution_calls;
  assert(wide_adc.read(sample) && sample == 1);
  assert(ArduinoStub.analog_read_resolution_calls == resolution_calls + 1);
  assert(ArduinoStub.last_analog_read_resolution == wide_adc_bits);

  nobro::ArduinoPwm narrow_pwm(4, 8);
  nobro::ArduinoPwm wide_pwm(5, 12);
  assert(narrow_pwm.begin() && wide_pwm.begin());
  assert(ArduinoStub.last_analog_write_resolution == 12);
  resolution_calls = ArduinoStub.analog_write_resolution_calls;
  assert(narrow_pwm.setDuty(1));
  assert(ArduinoStub.analog_write_resolution_calls == resolution_calls + 1);
  assert(ArduinoStub.last_analog_write_resolution == 8);
#if defined(ARDUINO_ARCH_ESP32)
  assert(ArduinoStub.last_analog_write_resolution_pin == 4);
#endif
  resolution_calls = ArduinoStub.analog_write_resolution_calls;
  assert(wide_pwm.setDuty(1));
  assert(ArduinoStub.analog_write_resolution_calls == resolution_calls + 1);
  assert(ArduinoStub.last_analog_write_resolution == 12);
#if defined(ARDUINO_ARCH_ESP32)
  assert(ArduinoStub.last_analog_write_resolution_pin == 5);
#endif
#endif
}

static void test_resumable_provider_report() {
  ProviderReport report;
  assert(report.begin(true, true, true, false, false, true));
  const size_t complete_length = report.remaining();

  CapturingShortStream stream;
  stream.write_limits[0] = 7;
  stream.write_limits[1] = 3;
  stream.write_limits[2] = 255;
  stream.write_limits[3] = 255;
  stream.write_limit_count = 4;
  nobro::ArduinoByteIo output(stream);

  assert(report.resume(output) == 7);
  assert(report.pending() && report.remaining() == complete_length - 7);
  // A new record cannot overwrite or interleave the retained suffix.
  assert(!report.begin(false, false, false, true, false, false));
  assert(report.resume(output) == 3);
  assert(report.pending() && report.remaining() == complete_length - 10);
  assert(report.resume(output) == nobro::ArduinoByteIo::MAX_WRITE_BYTES_PER_CALL);
  assert(report.pending());
  assert(report.resume(output) ==
         complete_length - 10 - nobro::ArduinoByteIo::MAX_WRITE_BYTES_PER_CALL);
  assert(!report.pending() && report.remaining() == 0);
  assert(report.resume(output) == 0);

  const char expected[] =
      "NOBRO-ARDUINO deadline=armed adc=sampled pwm=duty_requested "
      "i2c=not_exercised rfid=compile_only result=ok\r\n";
  assert(complete_length == sizeof(expected) - 1);
  assert(stream.captured_count == sizeof(expected) - 1);
  for (size_t i = 0; i < sizeof(expected) - 1; ++i)
    assert(stream.captured[i] == static_cast<uint8_t>(expected[i]));

  // Once complete, the bounded buffer can be reused for the next whole record.
  assert(report.begin(false, false, false, true, false, false));
}

static void test_spi_lifecycle() {
  ArduinoStub = ArduinoStubState();
  const uint8_t payload[3] = {1, 2, 3};
  uint8_t reply[3] = {};
  SPIClass bus;
  bus.transfer_xor = 0x55;

  nobro::ArduinoSpi invalid_frequency(9, bus, 0);
  assert(!invalid_frequency.begin() && !invalid_frequency.begun());
  assert(bus.begin_calls == 0 && ArduinoStub.pin_mode_calls == 0);

  nobro::ArduinoSpi device(10, bus);
  assert(!device.begun());
  assert(!device.transfer(payload, reply, 3));
  assert(bus.begin_transaction_calls == 0 && ArduinoStub.digital_write_calls == 0);
  assert(device.begin() && device.begun());
  assert(bus.begin_calls == 1 && ArduinoStub.pin_mode_calls == 1);
  assert(ArduinoStub.last_pin_mode_pin == 10 && ArduinoStub.last_pin_mode == OUTPUT);
  assert(ArduinoStub.digital_write_high_calls == 1 && ArduinoStub.digital_write_low_calls == 0);
  assert(device.begin() && bus.begin_calls == 1);
  assert(device.transfer(nullptr, nullptr, 0));
  assert(bus.begin_transaction_calls == 0);
  assert(!device.transfer(nullptr, reply, 1));
  assert(!device.transfer(payload, nullptr, 1));
  assert(bus.begin_transaction_calls == 0);
  assert(device.transfer(payload, reply, 3));
  assert(bus.begin_transaction_calls == 1 && bus.end_transaction_calls == 1);
  assert(bus.transfer_calls == 3);
  assert(reply[0] == (1 ^ 0x55) && reply[1] == (2 ^ 0x55) && reply[2] == (3 ^ 0x55));
  assert(ArduinoStub.digital_write_low_calls == 1);
  assert(ArduinoStub.digital_write_high_calls == 2);
  assert(ArduinoStub.last_digital_write_pin == 10 && ArduinoStub.last_digital_write_value == HIGH);
}

int main() {
  const uint8_t payload[3] = {1, 2, 3};
  test_adc_pwm_lifecycle_and_resolution();
  test_resolution_isolation();
  test_resumable_provider_report();
  test_spi_lifecycle();
  TwoWire wire;
  nobro::ArduinoI2c i2c(wire);

  assert(!i2c.begin(0));
  assert(i2c.lastError() == nobro::ARDUINO_I2C_INVALID_ARGUMENT);
  assert(!i2c.write(0x20, payload, 1));
  assert(i2c.lastError() == nobro::ARDUINO_I2C_NOT_BEGUN);
  assert(wire.begin_transmission_calls == 0 && wire.end_transmission_calls == 0);

  assert(i2c.begin(400000) && wire.begin_calls == 1);
  wire.next_write = 1;
  assert(!i2c.write(0x20, payload, 3));
  assert(i2c.lastError() == nobro::ARDUINO_I2C_SHORT_WRITE);
  assert(wire.begin_transmission_calls == 1 && wire.end_transmission_calls == 1);
  assert(wire.last_stop);

  wire.next_write = 3;
  wire.next_end_status = 2;
  assert(!i2c.write(0x20, payload, 3));
  assert(i2c.lastError() == nobro::ARDUINO_I2C_BUS_ERROR);
  assert(i2c.lastBusStatus() == 2 && wire.end_transmission_calls == 2);

  wire.next_end_status = 0;
  assert(i2c.write(0x20, payload, 3));
  assert(i2c.lastError() == nobro::ARDUINO_I2C_OK);

  const int transmissions_before_invalid_write_read = wire.begin_transmission_calls;
  assert(i2c.writeRead(0x20, payload, 1, nullptr, 1) == 0);
  assert(i2c.lastError() == nobro::ARDUINO_I2C_INVALID_ARGUMENT);
  assert(wire.begin_transmission_calls == transmissions_before_invalid_write_read);

  uint8_t read_buffer[3] = {};
  wire.next_request = 2;
  wire.read_count = 2;
  wire.read_values[0] = 9;
  wire.read_values[1] = 8;
  assert(i2c.read(0x20, read_buffer, 3) == 2);
  assert(i2c.lastError() == nobro::ARDUINO_I2C_SHORT_READ);
  assert(read_buffer[0] == 9 && read_buffer[1] == 8);

  FakeStream full;
  full.write_results[0] = 3;
  full.write_result_count = 1;
  nobro::ArduinoByteIo full_io(full);
  assert(full_io.writeAll(payload, 3) == 3);
  assert(full.write_calls == 1 && full.last_write_length == 3);

  FakeStream one_byte_progress;
  one_byte_progress.write_results[0] = 1;
  one_byte_progress.write_results[1] = 2;
  one_byte_progress.write_result_count = 2;
  nobro::ArduinoByteIo one_byte_io(one_byte_progress);
  assert(one_byte_io.writeAll(payload, 3) == 1);
  assert(one_byte_progress.write_calls ==
         nobro::ArduinoByteIo::MAX_WRITE_ATTEMPTS_PER_CALL);

  FakeStream zero_progress;
  zero_progress.write_results[0] = 0;
  zero_progress.write_result_count = 1;
  nobro::ArduinoByteIo zero_progress_io(zero_progress);
  assert(zero_progress_io.writeAll(payload, 3) == 0);
  assert(zero_progress.write_calls == 1);

  FakeStream over_report;
  over_report.write_results[0] = 4;
  over_report.write_result_count = 1;
  nobro::ArduinoByteIo over_report_io(over_report);
  assert(over_report_io.writeAll(payload, 3) == 0);
  assert(over_report.write_calls == 1);

  uint8_t long_payload[nobro::ArduinoByteIo::MAX_WRITE_BYTES_PER_CALL + 1] = {};
  FakeStream capped;
  capped.write_results[0] = 1;
  capped.write_results[1] = nobro::ArduinoByteIo::MAX_WRITE_BYTES_PER_CALL;
  capped.write_result_count = 2;
  nobro::ArduinoByteIo capped_io(capped);
  assert(capped_io.writeAll(long_payload, sizeof(long_payload)) == 1);
  assert(capped.write_calls == 1);
  assert(capped.last_write_length == nobro::ArduinoByteIo::MAX_WRITE_BYTES_PER_CALL);

  FakeStream readable;
  readable.read_values[0] = 7;
  readable.read_values[1] = -1;
  readable.read_count = 2;
  nobro::ArduinoByteIo readable_io(readable);
  assert(readable_io.readAvailable(read_buffer, 3) == 1 && read_buffer[0] == 7);
}
'''

PROVIDER_BASE_SOURCE = r'''
#define ARDUINO
#define NOBRO_ARDUINO_ENABLE_PROVIDERS
#include "NobroRTOS.h"
int main() {
  nobro::ArduinoDeadline deadline;
  return deadline.armAfterUs(1) ? 0 : 1;
}
'''


def compile_and_run(compiler: str, source_text: str, tmp: pathlib.Path,
                    stem: str, include_paths: list[pathlib.Path],
                    defines: tuple[str, ...] = ()) -> tuple[bool, str]:
    source = tmp / f"{stem}.cpp"
    binary = tmp / (f"{stem}.exe" if sys.platform == "win32" else stem)
    source.write_text(source_text, encoding="utf-8")
    command = [compiler, "-std=c++11", "-Wall", "-Wextra", "-Werror"]
    command.extend(f"-D{define}" for define in defines)
    for include in include_paths:
        command.extend(["-I", str(include)])
    command.extend([str(source), "-o", str(binary)])
    compiled = subprocess.run(command, capture_output=True, text=True)
    if compiled.returncode:
        return False, (compiled.stdout + compiled.stderr).strip()
    executed = subprocess.run([str(binary)], capture_output=True, text=True)
    if executed.returncode:
        return False, (executed.stdout + executed.stderr).strip()
    return True, ""


def compile_c_and_run(compiler: str, source_text: str, tmp: pathlib.Path,
                      stem: str, include_paths: list[pathlib.Path]) -> tuple[bool, str]:
    source = tmp / f"{stem}.c"
    binary = tmp / (f"{stem}.exe" if sys.platform == "win32" else stem)
    source.write_text(source_text, encoding="utf-8")
    command = [compiler, "-std=c11", "-Wall", "-Wextra", "-Werror"]
    for include in include_paths:
        command.extend(["-I", str(include)])
    command.extend([str(source), "-o", str(binary)])
    compiled = subprocess.run(command, capture_output=True, text=True)
    if compiled.returncode:
        return False, (compiled.stdout + compiled.stderr).strip()
    executed = subprocess.run([str(binary)], capture_output=True, text=True)
    if executed.returncode:
        return False, (executed.stdout + executed.stderr).strip()
    return True, ""


def main() -> int:
    compiler = shutil.which("g++") or shutil.which("g++.exe")
    c_compiler = shutil.which("gcc") or shutil.which("gcc.exe")
    if not compiler or not c_compiler:
        print("ARDUINO FACADE: FAIL (g++/gcc not found)")
        return 1
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        ok, output = compile_and_run(compiler, SOURCE, tmp_path, "facade", [HEADER])
        if not ok:
            print(output)
            print("ARDUINO FACADE: FAIL (contracts)")
            return 1
        ok, output = compile_and_run(
            compiler, WIRELESS_POLICY_SOURCE, tmp_path, "wireless_policy", [HEADER]
        )
        if not ok:
            print(output)
            print("ARDUINO FACADE: FAIL (adaptive wireless C++ policy)")
            return 1
        ok, output = compile_c_and_run(
            c_compiler, WIRELESS_C_SOURCE, tmp_path, "wireless_policy_c", [HEADER]
        )
        if not ok:
            print(output)
            print("ARDUINO FACADE: FAIL (adaptive wireless C policy)")
            return 1

        stub_path = tmp_path / "arduino_stubs"
        stub_path.mkdir()
        (stub_path / "Arduino.h").write_text(ARDUINO_STUB, encoding="utf-8")
        ok, output = compile_and_run(
            compiler, PROVIDER_BASE_SOURCE, tmp_path, "provider_base", [stub_path, HEADER]
        )
        if not ok:
            print(output)
            print("ARDUINO FACADE: FAIL (base provider requires an optional bus header)")
            return 1

        (stub_path / "Wire.h").write_text(WIRE_STUB, encoding="utf-8")
        (stub_path / "SPI.h").write_text(SPI_STUB, encoding="utf-8")
        provider_variants = (
            ("generic", ()),
            ("avr", ("ARDUINO_ARCH_AVR",)),
            ("esp32", ("ARDUINO_ARCH_ESP32",)),
            ("renesas", ("ARDUINO_ARCH_RENESAS",)),
            ("nrf52", ("ARDUINO_ARCH_NRF52",)),
        )
        for variant, defines in provider_variants:
            ok, output = compile_and_run(
                compiler, PROVIDER_SOURCE, tmp_path, f"providers_{variant}",
                [stub_path, HEADER, PROVIDER_EXAMPLE], defines
            )
            if not ok:
                print(output)
                print(f"ARDUINO FACADE: FAIL ({variant} provider lifecycle/resolution/I/O)")
                return 1
    print("ARDUINO FACADE: PASS (adaptive wireless C/C++ policies + NobroApp "
          "zero/overflow negatives + 5 executed provider "
          "architecture policies; ADC/PWM instance isolation + SPI/I2C lifecycle "
          "negatives + capped/resumable byte-I/O records)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
