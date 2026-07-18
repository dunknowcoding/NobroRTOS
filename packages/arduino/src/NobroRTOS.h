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

inline uint32_t hz(uint32_t rate) {
    return rate == 0 ? 0 : 1000000ul / rate;
}

enum TaskRole : uint8_t {
    CONTROL = 1,
    PERIODIC = 2,
    SENSOR = PERIODIC, /* compatibility alias */
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
    APP_INVALID_BUDGET,
    APP_INVALID_NAME,
    APP_DUPLICATE_TASK,
    APP_DUPLICATE_WIRE,
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
          ram_used_(3ul * 1024ul),
          error_(flash_limit == 0 || ram_limit == 0 ||
                         flash_limit < 12ul * 1024ul || ram_limit < 3ul * 1024ul
                     ? APP_RESOURCE_BUDGET
                     : APP_OK) {}

    TaskId task(const char *name, uint32_t period_us, TaskRole role = PERIODIC) {
        return add(name, role, period_us);
    }
    TaskId control(const char *name, uint32_t every_ms) {
        return task(name, milliseconds(every_ms), CONTROL);
    }
    TaskId sensor(const char *name, uint32_t every_ms) {
        return task(name, milliseconds(every_ms), PERIODIC);
    }
    TaskId service(const char *name, uint32_t every_ms) {
        return task(name, milliseconds(every_ms), SERVICE);
    }

    NobroApp &budget(TaskId id, uint32_t budget_us) {
        if (!contains(id)) return fail(APP_INVALID_TASK);
        if (budget_us == 0) return fail(APP_INVALID_BUDGET);
        tasks_[id.value].budget_us = budget_us;
        return *this;
    }
    NobroApp &memory(TaskId id, uint32_t flash_bytes, uint32_t ram_bytes) {
        if (!contains(id)) return fail(APP_INVALID_TASK);
        if (flash_bytes == 0 || ram_bytes == 0) return fail(APP_RESOURCE_BUDGET);
        uint32_t next_flash = 0;
        uint32_t next_ram = 0;
        if (!checked_replace(flash_used_, tasks_[id.value].flash_bytes,
                             flash_bytes, next_flash) ||
            !checked_replace(ram_used_, tasks_[id.value].ram_bytes,
                             ram_bytes, next_ram))
            return fail(APP_RESOURCE_BUDGET);
        tasks_[id.value].flash_bytes = flash_bytes;
        tasks_[id.value].ram_bytes = ram_bytes;
        flash_used_ = next_flash;
        ram_used_ = next_ram;
        return *this;
    }
    NobroApp &wire(TaskId from, TaskId to, uint8_t capacity = 1) {
        if (capacity == 0 || capacity > 64 || channel_count_ >= MaxChannels)
            return fail(APP_CHANNEL_CAPACITY);
        if (from.value == to.value) return fail(APP_INVALID_TASK);
        for (uint8_t i = 0; i < channel_count_; ++i) {
            if (channels_[i].from == from.value && channels_[i].to == to.value)
                return fail(APP_DUPLICATE_WIRE);
        }
        if (!contains(from) || !contains(to)) return fail(APP_INVALID_TASK);
        channels_[channel_count_].from = from.value;
        channels_[channel_count_].to = to.value;
        channels_[channel_count_].capacity = capacity;
        ++channel_count_;
        return *this;
    }
    NobroApp &connect(TaskId from, TaskId to) { return wire(from, to); }

    bool admit() {
        if (error_ != APP_OK) return false;
        for (uint8_t i = 0; i < task_count_; ++i) {
            if (tasks_[i].period_us == 0) return fail_bool(APP_INVALID_PERIOD);
            if (tasks_[i].budget_us == 0) return fail_bool(APP_INVALID_BUDGET);
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
        case APP_RESOURCE_BUDGET:
            return "task memory is zero, overflows, or exceeds the board profile";
        case APP_INVALID_BUDGET: return "task budget must be greater than zero";
        case APP_INVALID_NAME:
            return "task name must match [a-z][a-z0-9_-]{0,47}";
        case APP_DUPLICATE_TASK: return "task name is already declared";
        case APP_DUPLICATE_WIRE: return "wire is already declared";
        default: return "unknown application error";
        }
    }
    uint8_t taskCount() const { return task_count_; }
    uint8_t channelCount() const { return channel_count_; }
    uint8_t wireCount() const { return channel_count_; }
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
    struct Channel { uint8_t from; uint8_t to; uint8_t capacity; };

    TaskId add(const char *name, TaskRole role, uint32_t period) {
        if (!valid_name(name)) {
            fail(APP_INVALID_NAME);
            return invalid();
        }
        if (period == 0) {
            fail(APP_INVALID_PERIOD);
            return invalid();
        }
        for (uint8_t i = 0; i < task_count_; ++i) {
            if (same_name(tasks_[i].name, name)) {
                fail(APP_DUPLICATE_TASK);
                return invalid();
            }
        }
        if (task_count_ >= MaxTasks) {
            fail(APP_TASK_CAPACITY);
            return invalid();
        }
        const uint32_t flash = 1024;
        const uint32_t ram = 256;
        uint32_t next_flash = 0;
        uint32_t next_ram = 0;
        if (!checked_add(flash_used_, flash, next_flash) ||
            !checked_add(ram_used_, ram, next_ram)) {
            fail(APP_RESOURCE_BUDGET);
            return invalid();
        }
        Task &task = tasks_[task_count_];
        task.name = name;
        task.role = (uint8_t)role;
        task.period_us = period;
        task.budget_us = period / 10ul;
        if (task.budget_us == 0) task.budget_us = 1;
        task.flash_bytes = flash;
        task.ram_bytes = ram;
        flash_used_ = next_flash;
        ram_used_ = next_ram;
        TaskId result = {task_count_};
        ++task_count_;
        return result;
    }
    bool contains(TaskId id) const { return id.valid() && id.value < task_count_; }
    static uint32_t milliseconds(uint32_t value) {
        return value > (0xFFFFFFFFul / 1000ul) ? 0ul : value * 1000ul;
    }
    static bool same_name(const char *left, const char *right) {
        if (left == 0 || right == 0) return left == right;
        while (*left != '\0' && *left == *right) {
            ++left;
            ++right;
        }
        return *left == *right;
    }
    static bool valid_name(const char *name) {
        if (name == 0 || name[0] < 'a' || name[0] > 'z') return false;
        uint8_t length = 0;
        for (; name[length] != '\0'; ++length) {
            const char value = name[length];
            if (length >= 48 ||
                !((value >= 'a' && value <= 'z') ||
                  (value >= '0' && value <= '9') || value == '_' || value == '-'))
                return false;
        }
        return length != 0;
    }
    static bool checked_add(uint32_t current, uint32_t increment, uint32_t &result) {
        if (increment > 0xFFFFFFFFul - current) return false;
        result = current + increment;
        return true;
    }
    static bool checked_replace(uint32_t current, uint32_t previous,
                                uint32_t replacement, uint32_t &result) {
        if (current < previous) return false;
        return checked_add(current - previous, replacement, result);
    }
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

#if defined(ARDUINO) && defined(__cplusplus) && \
    defined(NOBRO_ARDUINO_ENABLE_PROVIDERS)
#include "NobroArduinoProviders.h"
#endif

#endif /* NOBRO_RTOS_ARDUINO_H */
