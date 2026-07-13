/*
 * Arduino compatibility include for NobroRTOS.
 *
 * The canonical C ABI headers live in bindings/c/include and are vendored into
 * this library by tools/package_arduino.py --sync (drift-gated in CI). This
 * header keeps the Arduino package thin while repository-local examples and
 * library consumers can include <NobroRTOS.h>.
 */

#ifndef NOBRO_RTOS_ARDUINO_H
#define NOBRO_RTOS_ARDUINO_H

#include "nobro_rtos.h"

#ifdef __cplusplus

namespace nobro {

enum TaskRole : uint8_t {
    CONTROL = 1,
    SENSOR = 2,
    SERVICE = 3,
};

struct TaskId {
    uint8_t value;
    bool valid() const { return value != 0xFFu; }
};

enum AppError : uint8_t {
    APP_OK = 0,
    APP_TASK_CAPACITY,
    APP_CHANNEL_CAPACITY,
    APP_INVALID_TASK,
    APP_INVALID_PERIOD,
    APP_BUDGET_EXCEEDS_PERIOD,
    APP_RESOURCE_BUDGET,
};

/* Allocation-free Arduino declaration and admission preview. The Rust firmware path
 * remains authoritative for execution; this facade removes raw ABI/report boilerplate
 * and catches common contract errors in an ordinary sketch. */
template <uint8_t MaxTasks = 8, uint8_t MaxChannels = 8>
class NobroApp {
public:
    NobroApp(uint32_t flash_limit = 128ul * 1024ul,
             uint32_t ram_limit = 32ul * 1024ul)
        : task_count_(0), channel_count_(0), flash_limit_(flash_limit),
          ram_limit_(ram_limit), flash_used_(12ul * 1024ul),
          ram_used_(3ul * 1024ul), error_(APP_OK) {}

    TaskId control(const char *name, uint32_t every_ms) {
        return add(name, CONTROL, every_ms, 2048, 512, 5);
    }
    TaskId sensor(const char *name, uint32_t every_ms) {
        return add(name, SENSOR, every_ms, 1024, 256, 10);
    }
    TaskId service(const char *name, uint32_t every_ms) {
        return add(name, SERVICE, every_ms, 1024, 256, 20);
    }

    NobroApp &budget(TaskId id, uint32_t budget_us) {
        if (!contains(id)) return fail(APP_INVALID_TASK);
        tasks_[id.value].budget_us = budget_us;
        return *this;
    }
    NobroApp &memory(TaskId id, uint32_t flash_bytes, uint32_t ram_bytes) {
        if (!contains(id)) return fail(APP_INVALID_TASK);
        flash_used_ -= tasks_[id.value].flash_bytes;
        ram_used_ -= tasks_[id.value].ram_bytes;
        tasks_[id.value].flash_bytes = flash_bytes;
        tasks_[id.value].ram_bytes = ram_bytes;
        flash_used_ += flash_bytes;
        ram_used_ += ram_bytes;
        return *this;
    }
    NobroApp &connect(TaskId from, TaskId to) {
        if (!contains(from) || !contains(to) || from.value == to.value)
            return fail(APP_INVALID_TASK);
        if (channel_count_ >= MaxChannels) return fail(APP_CHANNEL_CAPACITY);
        channels_[channel_count_].from = from.value;
        channels_[channel_count_].to = to.value;
        ++channel_count_;
        return *this;
    }

    bool admit() {
        if (error_ != APP_OK) return false;
        for (uint8_t i = 0; i < task_count_; ++i) {
            if (tasks_[i].period_us == 0) return fail_bool(APP_INVALID_PERIOD);
            if (tasks_[i].budget_us > tasks_[i].period_us)
                return fail_bool(APP_BUDGET_EXCEEDS_PERIOD);
        }
        if (flash_used_ > flash_limit_ || ram_used_ > ram_limit_)
            return fail_bool(APP_RESOURCE_BUDGET);
        return true;
    }

    AppError error() const { return error_; }
    const char *errorText() const {
        switch (error_) {
        case APP_OK: return "ready";
        case APP_TASK_CAPACITY: return "too many tasks; raise NobroApp task capacity";
        case APP_CHANNEL_CAPACITY: return "too many channels; raise NobroApp channel capacity";
        case APP_INVALID_TASK: return "channel or override names an invalid task";
        case APP_INVALID_PERIOD: return "task period must be greater than zero";
        case APP_BUDGET_EXCEEDS_PERIOD: return "task budget exceeds its period";
        case APP_RESOURCE_BUDGET: return "declared task memory exceeds the board profile";
        default: return "unknown application error";
        }
    }
    uint8_t taskCount() const { return task_count_; }
    uint8_t channelCount() const { return channel_count_; }
    uint32_t flashUsed() const { return flash_used_; }
    uint32_t ramUsed() const { return ram_used_; }

private:
    struct Task {
        const char *name;
        uint32_t period_us;
        uint32_t budget_us;
        uint32_t flash_bytes;
        uint32_t ram_bytes;
        uint8_t role;
    };
    struct Channel { uint8_t from; uint8_t to; };

    TaskId add(const char *name, TaskRole role, uint32_t every_ms,
               uint32_t flash, uint32_t ram, uint8_t divisor) {
        if (task_count_ >= MaxTasks) {
            fail(APP_TASK_CAPACITY);
            return invalid();
        }
        uint32_t period = every_ms > (0xFFFFFFFFul / 1000ul)
            ? 0ul : every_ms * 1000ul;
        Task &task = tasks_[task_count_];
        task.name = name;
        task.role = (uint8_t)role;
        task.period_us = period;
        task.budget_us = period / divisor;
        task.flash_bytes = flash;
        task.ram_bytes = ram;
        flash_used_ += flash;
        ram_used_ += ram;
        TaskId result = {task_count_};
        ++task_count_;
        return result;
    }
    bool contains(TaskId id) const { return id.valid() && id.value < task_count_; }
    static TaskId invalid() { TaskId id = {0xFFu}; return id; }
    NobroApp &fail(AppError error) { if (error_ == APP_OK) error_ = error; return *this; }
    bool fail_bool(AppError error) { fail(error); return false; }

    Task tasks_[MaxTasks];
    Channel channels_[MaxChannels];
    uint8_t task_count_;
    uint8_t channel_count_;
    uint32_t flash_limit_;
    uint32_t ram_limit_;
    uint32_t flash_used_;
    uint32_t ram_used_;
    AppError error_;
};

} // namespace nobro
#endif

#endif /* NOBRO_RTOS_ARDUINO_H */

#ifdef ARDUINO
#include "NobroArduinoProviders.h"
#endif
