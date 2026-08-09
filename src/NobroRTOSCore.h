// SPDX-License-Identifier: GPL-3.0-only
#ifndef NOBRO_RTOS_CORE_ARDUINO_H
#define NOBRO_RTOS_CORE_ARDUINO_H

#include <stddef.h>
#include <stdint.h>

namespace nobro {
namespace core {

typedef void (*TaskCallback)(void *context);

struct Task {
    uint16_t id;
    uint8_t priority;
    uint32_t period_us;
    uint32_t phase_us;
    uint32_t finish_deadline_us;
    uint32_t wcet_us;
    TaskCallback callback;
    void *context;

    static Task periodic(uint16_t id,
                         uint8_t priority,
                         uint32_t period_us,
                         uint32_t finish_deadline_us,
                         uint32_t wcet_us,
                         TaskCallback callback,
                         void *context = 0,
                         uint32_t phase_us = 0)
    {
        Task task = {id, priority, period_us, phase_us, finish_deadline_us,
                     wcet_us, callback, context};
        return task;
    }

    static Task event(uint16_t id,
                      uint8_t priority,
                      uint32_t wcet_us,
                      TaskCallback callback,
                      void *context = 0)
    {
        Task task = {id, priority, 0, 0, 0, wcet_us, callback, context};
        return task;
    }
};

enum AdmissionResult {
    ADMITTED = 0,
    EMPTY_WORKLOAD,
    TOO_MANY_TASKS,
    DUPLICATE_ID,
    DUPLICATE_PRIORITY,
    INVALID_PRIORITY,
    INVALID_TIMING,
    INVALID_CALLBACK,
    UTILIZATION_EXCEEDED,
    DEADLINE_MISS
};

template <size_t TaskCount>
class Scheduler {
public:
    static const size_t MAX_TASKS = 32;
    static const uint32_t MAX_WRAP_SAFE_INTERVAL_US = 0x7ffffffful;

    Scheduler()
        : ready_priorities_(0), admitted_(false), result_(EMPTY_WORKLOAD)
    {
        static_assert(TaskCount > 0, "NobroRTOS Core requires at least one task");
        static_assert(TaskCount <= MAX_TASKS,
                      "NobroRTOS Core supports at most 32 tasks");
        for (size_t index = 0; index < MAX_TASKS; ++index) {
            priority_to_task_[index] = 0xffu;
        }
    }

    AdmissionResult begin(const Task (&tasks)[TaskCount], uint32_t epoch_us)
    {
        admitted_ = false;
        ready_priorities_ = 0;
        for (size_t index = 0; index < TaskCount; ++index) {
            tasks_[index] = tasks[index];
        }
        result_ = validate();
        if (result_ != ADMITTED) {
            return result_;
        }
        for (size_t index = 0; index < MAX_TASKS; ++index) {
            priority_to_task_[index] = 0xffu;
        }
        for (size_t index = 0; index < TaskCount; ++index) {
            priority_to_task_[tasks_[index].priority] =
                static_cast<uint8_t>(index);
            next_release_us_[index] = epoch_us + tasks_[index].phase_us;
        }
        admitted_ = true;
        return result_;
    }

    uint8_t releaseDue(uint32_t now_us)
    {
        if (!admitted_) {
            return 0;
        }
        const uint32_t before = ready_priorities_;
        for (size_t index = 0; index < TaskCount; ++index) {
            const Task &task = tasks_[index];
            if (task.period_us == 0) {
                continue;
            }
            const uint32_t release = next_release_us_[index];
            if (static_cast<uint32_t>(now_us - release) < 0x80000000ul) {
                ready_priorities_ |= (1ul << task.priority);
                const uint32_t elapsed = now_us - release;
                const uint32_t periods = elapsed / task.period_us + 1ul;
                next_release_us_[index] = release + periods * task.period_us;
            }
        }
        return popcount32(ready_priorities_ & ~before);
    }

    bool markReady(size_t task_index)
    {
        if (!admitted_ || task_index >= TaskCount) {
            return false;
        }
        ready_priorities_ |= (1ul << tasks_[task_index].priority);
        return true;
    }

    bool markReadyById(uint16_t task_id)
    {
        for (size_t index = 0; index < TaskCount; ++index) {
            if (tasks_[index].id == task_id) {
                return markReady(index);
            }
        }
        return false;
    }

    bool takeNext(size_t &task_index)
    {
        if (!admitted_ || ready_priorities_ == 0) {
            return false;
        }
        uint8_t priority = 0;
        while ((ready_priorities_ & (1ul << priority)) == 0) {
            ++priority;
        }
        ready_priorities_ &= ~(1ul << priority);
        const uint8_t mapped = priority_to_task_[priority];
        if (mapped == 0xffu) {
            return false;
        }
        task_index = mapped;
        return true;
    }

    bool runNext()
    {
        size_t task_index = 0;
        if (!takeNext(task_index)) {
            return false;
        }
        tasks_[task_index].callback(tasks_[task_index].context);
        return true;
    }

    uint8_t runReady(uint8_t dispatch_limit = static_cast<uint8_t>(TaskCount))
    {
        uint8_t dispatched = 0;
        while (dispatched < dispatch_limit && runNext()) {
            ++dispatched;
        }
        return dispatched;
    }

    bool nextRelease(uint32_t now_us, uint32_t &release_us) const
    {
        if (!admitted_) {
            return false;
        }
        bool found = false;
        uint32_t nearest = 0;
        for (size_t index = 0; index < TaskCount; ++index) {
            if (tasks_[index].period_us == 0) {
                continue;
            }
            const uint32_t raw = next_release_us_[index] - now_us;
            const uint32_t distance = raw < 0x80000000ul ? raw : 0;
            if (!found || distance < nearest) {
                nearest = distance;
                found = true;
            }
        }
        if (found) {
            release_us = now_us + nearest;
        }
        return found;
    }

    bool isIdle() const { return ready_priorities_ == 0; }
    bool isAdmitted() const { return admitted_; }
    AdmissionResult admissionResult() const { return result_; }
    const Task &task(size_t index) const { return tasks_[index]; }

private:
    static uint8_t popcount32(uint32_t value)
    {
        uint8_t count = 0;
        while (value != 0) {
            value &= value - 1ul;
            ++count;
        }
        return count;
    }

    AdmissionResult validate() const
    {
        uint32_t priority_bits = 0;
        uint64_t utilization_q32 = 0;
        for (size_t index = 0; index < TaskCount; ++index) {
            const Task &task = tasks_[index];
            if (task.priority >= MAX_TASKS) {
                return INVALID_PRIORITY;
            }
            const uint32_t priority_bit = 1ul << task.priority;
            if ((priority_bits & priority_bit) != 0) {
                return DUPLICATE_PRIORITY;
            }
            priority_bits |= priority_bit;
            if (task.callback == 0) {
                return INVALID_CALLBACK;
            }
            for (size_t prior = 0; prior < index; ++prior) {
                if (tasks_[prior].id == task.id) {
                    return DUPLICATE_ID;
                }
            }
            if (task.wcet_us == 0 ||
                task.wcet_us > MAX_WRAP_SAFE_INTERVAL_US) {
                return INVALID_TIMING;
            }
            if (task.period_us == 0) {
                if (task.phase_us != 0 || task.finish_deadline_us != 0) {
                    return INVALID_TIMING;
                }
            } else {
                if (task.period_us > MAX_WRAP_SAFE_INTERVAL_US ||
                    task.phase_us >= task.period_us ||
                    task.finish_deadline_us == 0 ||
                    task.finish_deadline_us > task.period_us ||
                    task.wcet_us > task.finish_deadline_us) {
                    return INVALID_TIMING;
                }
                const uint64_t scaled =
                    static_cast<uint64_t>(task.wcet_us) << 32;
                utilization_q32 +=
                    (scaled + task.period_us - 1ul) / task.period_us;
                if (utilization_q32 > (static_cast<uint64_t>(1) << 32)) {
                    return UTILIZATION_EXCEEDED;
                }
            }
        }

        for (size_t index = 0; index < TaskCount; ++index) {
            const Task &task = tasks_[index];
            if (task.period_us == 0) {
                continue;
            }
            uint64_t blocking = 0;
            for (size_t other = 0; other < TaskCount; ++other) {
                const Task &candidate = tasks_[other];
                if (candidate.priority > task.priority &&
                    candidate.wcet_us > blocking) {
                    blocking = candidate.wcet_us;
                }
            }
            uint64_t response = task.wcet_us + blocking;
            if (response > task.finish_deadline_us) {
                return DEADLINE_MISS;
            }
            for (;;) {
                uint64_t updated = task.wcet_us + blocking;
                for (size_t other = 0; other < TaskCount; ++other) {
                    const Task &higher = tasks_[other];
                    if (higher.period_us != 0 &&
                        higher.priority < task.priority) {
                        const uint64_t jobs =
                            (response + higher.period_us - 1ul) /
                            higher.period_us;
                        updated += jobs * higher.wcet_us;
                    }
                }
                if (updated == response) {
                    break;
                }
                if (updated > task.finish_deadline_us || updated < response) {
                    return DEADLINE_MISS;
                }
                response = updated;
            }
        }
        return ADMITTED;
    }

    Task tasks_[TaskCount];
    uint32_t next_release_us_[TaskCount];
    uint8_t priority_to_task_[MAX_TASKS];
    uint32_t ready_priorities_;
    bool admitted_;
    AdmissionResult result_;
};

}  // namespace core
}  // namespace nobro

#endif
