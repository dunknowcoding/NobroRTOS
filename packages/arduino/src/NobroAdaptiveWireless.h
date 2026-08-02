#ifndef NOBRO_ADAPTIVE_WIRELESS_H
#define NOBRO_ADAPTIVE_WIRELESS_H

#include <string.h>

#include "nobro_wireless.h"

namespace nobro {

/* Small C++ builders for the portable C policy. Queue storage and the selected
 * radio stack remain explicit, so disabled adaptive traffic adds no runtime. */
class WirelessPolicy {
public:
    static WirelessPolicy responsive(uint16_t slots, uint16_t payload_bytes) {
        return WirelessPolicy(nobro_wireless_responsive_policy(slots, payload_bytes));
    }

    static WirelessPolicy lowEnergy(
        uint16_t slots,
        uint16_t payload_bytes,
        uint64_t batch_window_us) {
        return WirelessPolicy(
            nobro_wireless_low_energy_policy(slots, payload_bytes, batch_window_us));
    }

    WirelessPolicy &callerPool() {
        value_.storage_mode = NOBRO_WIRELESS_STORAGE_CALLER_POOL;
        return *this;
    }

    WirelessPolicy &heapBacked() {
        value_.storage_mode = NOBRO_WIRELESS_STORAGE_HEAP;
        return *this;
    }

    bool valid() const { return nobro_wireless_policy_valid(&value_); }
    const nobro_wireless_adaptive_policy_t &native() const { return value_; }

private:
    explicit WirelessPolicy(nobro_wireless_adaptive_policy_t value) : value_(value) {}
    nobro_wireless_adaptive_policy_t value_;
};

class WirelessMessage {
public:
    static WirelessMessage bestEffort(uint64_t now_us, uint64_t expires_at_us) {
        return WirelessMessage(nobro_wireless_best_effort_message(now_us, expires_at_us));
    }

    static WirelessMessage urgent(uint64_t now_us, uint64_t deadline_us) {
        return WirelessMessage(nobro_wireless_urgent_message(now_us, deadline_us));
    }

    WirelessMessage &deadline(uint64_t deadline_us) {
        value_.deadline_us = deadline_us;
        return *this;
    }

    WirelessMessage &priority(uint8_t priority) {
        value_.priority = priority;
        return *this;
    }

    WirelessMessage &batchable(bool value) {
        value_.batchable = value;
        return *this;
    }

    bool valid() const { return nobro_wireless_message_valid(&value_); }
    const nobro_wireless_message_contract_t &native() const { return value_; }

private:
    explicit WirelessMessage(nobro_wireless_message_contract_t value) : value_(value) {}
    nobro_wireless_message_contract_t value_;
};

/* Bounded lifecycle retry state for joins, reconnects, and other operations
 * that happen outside the message queue. It owns no timer and allocates no
 * memory; the application decides how to idle until nextAttemptUs(). */
class WirelessRecovery {
public:
    static WirelessRecovery exponential(
        uint8_t max_attempts,
        uint64_t initial_backoff_us,
        uint64_t max_backoff_us) {
        const nobro_wireless_retry_policy_t policy = {
            max_attempts, initial_backoff_us, max_backoff_us};
        return WirelessRecovery(policy);
    }

    bool valid() const {
        return nobro_wireless_retry_policy_valid(&policy_);
    }

    void reset() { nobro_wireless_recovery_reset(&state_); }
    bool ready(uint64_t now_us) const {
        return nobro_wireless_recovery_ready(&state_, now_us);
    }
    bool failed(uint64_t now_us) {
        return nobro_wireless_recovery_failed(&state_, &policy_, now_us);
    }
    uint8_t failedAttempts() const { return state_.failed_attempts; }
    uint64_t nextAttemptUs() const { return state_.next_attempt_us; }

private:
    explicit WirelessRecovery(nobro_wireless_retry_policy_t policy)
        : policy_(policy) {
        nobro_wireless_recovery_reset(&state_);
    }

    nobro_wireless_retry_policy_t policy_;
    nobro_wireless_recovery_state_t state_;
};

struct WirelessTicket {
    uint32_t id;
    bool valid;
};

enum WirelessEventKind {
    WIRELESS_EMPTY,
    WIRELESS_IDLE_UNTIL,
    WIRELESS_DELIVERED,
    WIRELESS_EXPIRED,
    WIRELESS_RETRY_AT,
    WIRELESS_RETRY_EXHAUSTED,
    WIRELESS_REJECTED
};

struct WirelessEvent {
    WirelessEventKind kind;
    uint32_t id;
    uint64_t atUs;
    nobro_wireless_send_result_t result;
};

typedef nobro_wireless_send_result_t (*WirelessSend)(
    const uint8_t *payload,
    size_t length,
    uint64_t expires_at_us,
    void *context);

/* Ready-to-use fixed queue for Arduino sketches. It performs no allocation and
 * calls the selected transport only from service(), never from enqueue() or an ISR. */
template <size_t SLOTS, size_t BYTES>
class AdaptiveWirelessQueue {
public:
    static_assert(SLOTS > 0, "adaptive wireless queue requires at least one slot");
    static_assert(BYTES > 0, "adaptive wireless queue requires nonzero payload capacity");

    explicit AdaptiveWirelessQueue(const WirelessPolicy &policy)
        : policy_(policy.native()), next_id_(1), length_(0), valid_(
              policy.valid() &&
              policy.native().storage_mode == NOBRO_WIRELESS_STORAGE_FIXED &&
              policy.native().queue_slots == SLOTS &&
              policy.native().payload_bytes_per_slot == BYTES) {
        memset(slots_, 0, sizeof(slots_));
        memset(&diagnostics_, 0, sizeof(diagnostics_));
    }

    bool valid() const { return valid_; }
    size_t size() const { return length_; }
    bool empty() const { return length_ == 0; }
    static size_t capacity() { return SLOTS; }
    static size_t payloadCapacity() { return BYTES; }
    static size_t reservedBytes() { return sizeof(AdaptiveWirelessQueue<SLOTS, BYTES>); }

    WirelessTicket enqueue(
        const uint8_t *payload,
        size_t length,
        const WirelessMessage &message) {
        if (!valid_ || !message.valid() || length > BYTES ||
            (payload == NULL && length != 0)) {
            return invalidTicket();
        }
        increment(diagnostics_.offered_messages);
        add(diagnostics_.offered_bytes, length);
        size_t index = SLOTS;
        for (size_t candidate = 0; candidate < SLOTS; ++candidate) {
            if (!slots_[candidate].occupied) {
                index = candidate;
                break;
            }
        }
        if (index == SLOTS) {
            increment(diagnostics_.backpressure_rejections);
            return invalidTicket();
        }
        Slot &slot = slots_[index];
        if (length != 0) {
            memcpy(slot.payload, payload, length);
        }
        slot.length = static_cast<uint16_t>(length);
        slot.contract = message.native();
        slot.id = allocateId();
        slot.attempts = 0;
        slot.next_attempt_us = slot.contract.offered_at_us;
        slot.deadline_recorded = false;
        slot.occupied = true;
        ++length_;
        WirelessTicket ticket = {slot.id, true};
        return ticket;
    }

    WirelessTicket enqueueBestEffortFor(
        const uint8_t *payload,
        size_t length,
        uint64_t now_us,
        uint64_t useful_for_us) {
        return enqueue(payload, length, WirelessMessage::bestEffort(
            now_us, saturatingAdd(now_us, useful_for_us)));
    }

    WirelessTicket enqueueUrgentWithin(
        const uint8_t *payload,
        size_t length,
        uint64_t now_us,
        uint64_t within_us) {
        return enqueue(payload, length, WirelessMessage::urgent(
            now_us, saturatingAdd(now_us, within_us)));
    }

    bool cancel(WirelessTicket ticket) {
        if (!ticket.valid) return false;
        for (size_t index = 0; index < SLOTS; ++index) {
            if (slots_[index].occupied && slots_[index].id == ticket.id) {
                release(index);
                increment(diagnostics_.cancelled_messages);
                return true;
            }
        }
        return false;
    }

    WirelessEvent service(uint64_t now_us, WirelessSend send, void *context = NULL) {
        if (!valid_ || send == NULL) {
            return event(WIRELESS_REJECTED, 0, now_us,
                         NOBRO_WIRELESS_SEND_BACKEND_REJECTED);
        }
        const size_t expired = oldestExpired(now_us);
        if (expired != SLOTS) {
            const uint32_t id = slots_[expired].id;
            recordDeadlineMiss(expired, now_us);
            release(expired);
            increment(diagnostics_.expired_messages);
            return event(WIRELESS_EXPIRED, id, now_us, NOBRO_WIRELESS_SEND_OK);
        }
        const size_t index = bestReady(now_us);
        if (index == SLOTS) {
            const uint64_t next = nextReadyTime();
            return length_ == 0
                ? event(WIRELESS_EMPTY, 0, 0, NOBRO_WIRELESS_SEND_OK)
                : event(WIRELESS_IDLE_UNTIL, 0, next, NOBRO_WIRELESS_SEND_OK);
        }
        recordDeadlineMiss(index, now_us);
        Slot &slot = slots_[index];
        const uint32_t id = slot.id;
        const uint64_t expires = slot.contract.expires_at_us;
        ++slot.attempts;
        const nobro_wireless_send_result_t result =
            send(slot.payload, slot.length, expires, context);
        if (result == NOBRO_WIRELESS_SEND_OK) {
            const uint64_t latency = now_us - slot.contract.offered_at_us;
            increment(diagnostics_.delivered_messages);
            add(diagnostics_.delivered_bytes, slot.length);
            add(diagnostics_.latency_sum_us, latency);
            if (latency > diagnostics_.latency_max_us) {
                diagnostics_.latency_max_us = latency;
            }
            release(index);
            return event(WIRELESS_DELIVERED, id, now_us, result);
        }
        if (result == NOBRO_WIRELESS_SEND_LINK_DOWN ||
            result == NOBRO_WIRELESS_SEND_WINDOW_EXHAUSTED) {
            --slot.attempts;
            if (result == NOBRO_WIRELESS_SEND_LINK_DOWN) {
                increment(diagnostics_.link_down_deferrals);
            } else {
                increment(diagnostics_.window_deferrals);
            }
            const uint64_t retry_at = deferWithoutAttempt(slot, now_us);
            return event(WIRELESS_RETRY_AT, id, retry_at, result);
        }
        if (result == NOBRO_WIRELESS_SEND_PAYLOAD_TOO_LARGE ||
            result == NOBRO_WIRELESS_SEND_DEADLINE_ELAPSED) {
            release(index);
            return event(WIRELESS_REJECTED, id, now_us, result);
        }
        increment(diagnostics_.backend_rejections);
        increment(diagnostics_.retry_attempts);
        if (slot.attempts >= policy_.retry.max_attempts) {
            release(index);
            increment(diagnostics_.retry_exhaustions);
            return event(WIRELESS_RETRY_EXHAUSTED, id, now_us, result);
        }
        const uint64_t retry_at = minimum(
            saturatingAdd(now_us, retryDelay(slot.attempts)), expires);
        slot.next_attempt_us = retry_at;
        return event(WIRELESS_RETRY_AT, id, retry_at, result);
    }

    uint16_t serviceBatch(
        uint64_t now_us,
        WirelessSend send,
        void *context = NULL) {
        if (!valid_ || send == NULL) return 0;
        uint16_t delivered = 0;
        uint16_t serviced = 0;
        while (serviced < policy_.max_batch_messages) {
            const WirelessEvent outcome = service(now_us, send, context);
            if (outcome.kind == WIRELESS_DELIVERED) {
                ++delivered;
                ++serviced;
            } else if (outcome.kind == WIRELESS_EXPIRED ||
                       outcome.kind == WIRELESS_RETRY_EXHAUSTED ||
                       outcome.kind == WIRELESS_REJECTED) {
                ++serviced;
            } else {
                break;
            }
        }
        if (serviced != 0) increment(diagnostics_.radio_wake_batches);
        return delivered;
    }

    void notifyCompletion(void (*wake)(void *), void *context = NULL) {
        increment(diagnostics_.completion_wakes);
        if (wake != NULL) wake(context);
    }

    const nobro_wireless_adaptive_diagnostics_t &diagnostics() const {
        return diagnostics_;
    }

    uint64_t nextServiceUs() const {
        return nextReadyTime();
    }

private:
    struct Slot {
        uint8_t payload[BYTES];
        uint64_t next_attempt_us;
        nobro_wireless_message_contract_t contract;
        uint32_t id;
        uint16_t length;
        uint8_t attempts;
        bool occupied;
        bool deadline_recorded;
    };

    Slot slots_[SLOTS];
    nobro_wireless_adaptive_policy_t policy_;
    nobro_wireless_adaptive_diagnostics_t diagnostics_;
    uint32_t next_id_;
    size_t length_;
    bool valid_;

    static WirelessTicket invalidTicket() {
        WirelessTicket ticket = {0, false};
        return ticket;
    }

    static WirelessEvent event(
        WirelessEventKind kind,
        uint32_t id,
        uint64_t at_us,
        nobro_wireless_send_result_t result) {
        WirelessEvent value = {kind, id, at_us, result};
        return value;
    }

    static uint64_t minimum(uint64_t left, uint64_t right) {
        return left < right ? left : right;
    }

    static uint64_t saturatingAdd(uint64_t left, uint64_t right) {
        return UINT64_MAX - left < right ? UINT64_MAX : left + right;
    }

    static void increment(uint32_t &value) {
        if (value != UINT32_MAX) ++value;
    }

    static void add(uint64_t &value, uint64_t amount) {
        value = saturatingAdd(value, amount);
    }

    uint64_t retryDelay(uint8_t failed_attempts) const {
        if (failed_attempts == 0 || policy_.retry.initial_backoff_us == 0) return 0;
        uint64_t delay = policy_.retry.initial_backoff_us;
        for (uint8_t shift = 1; shift < failed_attempts; ++shift) {
            if (delay >= policy_.retry.max_backoff_us || delay > UINT64_MAX / 2) {
                return policy_.retry.max_backoff_us;
            }
            delay *= 2;
        }
        return minimum(delay, policy_.retry.max_backoff_us);
    }

    uint32_t allocateId() {
        for (;;) {
            const uint32_t id = next_id_ == 0 ? 1 : next_id_;
            next_id_ = id == UINT32_MAX ? 1 : id + 1;
            bool used = false;
            for (size_t index = 0; index < SLOTS; ++index) {
                used = used || (slots_[index].occupied && slots_[index].id == id);
            }
            if (!used) return id;
        }
    }

    void release(size_t index) {
        memset(&slots_[index], 0, sizeof(slots_[index]));
        if (length_ != 0) --length_;
    }

    void recordDeadlineMiss(size_t index, uint64_t now_us) {
        Slot &slot = slots_[index];
        if (now_us > slot.contract.deadline_us && !slot.deadline_recorded) {
            slot.deadline_recorded = true;
            increment(diagnostics_.deadline_misses);
        }
    }

    uint64_t readyAt(const Slot &slot) const {
        uint64_t batch_at = slot.contract.offered_at_us;
        if (slot.contract.batchable) {
            batch_at = minimum(
                minimum(saturatingAdd(slot.contract.offered_at_us,
                                      policy_.batch_window_us),
                        slot.contract.deadline_us),
                slot.contract.expires_at_us);
        }
        return slot.next_attempt_us > batch_at ? slot.next_attempt_us : batch_at;
    }

    size_t oldestExpired(uint64_t now_us) const {
        size_t selected = SLOTS;
        for (size_t index = 0; index < SLOTS; ++index) {
            const Slot &slot = slots_[index];
            if (!slot.occupied || now_us <= slot.contract.expires_at_us) continue;
            if (selected == SLOTS ||
                slot.contract.expires_at_us < slots_[selected].contract.expires_at_us ||
                (slot.contract.expires_at_us == slots_[selected].contract.expires_at_us &&
                 slot.id < slots_[selected].id)) {
                selected = index;
            }
        }
        return selected;
    }

    size_t bestReady(uint64_t now_us) const {
        size_t selected = SLOTS;
        for (size_t index = 0; index < SLOTS; ++index) {
            const Slot &slot = slots_[index];
            if (!slot.occupied || readyAt(slot) > now_us) continue;
            if (selected == SLOTS ||
                slot.contract.priority > slots_[selected].contract.priority ||
                (slot.contract.priority == slots_[selected].contract.priority &&
                 (slot.contract.deadline_us < slots_[selected].contract.deadline_us ||
                  (slot.contract.deadline_us == slots_[selected].contract.deadline_us &&
                   (slot.contract.offered_at_us < slots_[selected].contract.offered_at_us ||
                    (slot.contract.offered_at_us == slots_[selected].contract.offered_at_us &&
                     slot.id < slots_[selected].id)))))) {
                selected = index;
            }
        }
        return selected;
    }

    uint64_t nextReadyTime() const {
        uint64_t next = UINT64_MAX;
        for (size_t index = 0; index < SLOTS; ++index) {
            if (slots_[index].occupied) {
                next = minimum(next, minimum(readyAt(slots_[index]),
                                             slots_[index].contract.expires_at_us));
            }
        }
        return next;
    }

    uint64_t deferWithoutAttempt(Slot &slot, uint64_t now_us) {
        const uint64_t delay = policy_.retry.initial_backoff_us == 0
            ? 1 : policy_.retry.initial_backoff_us;
        const uint64_t retry_at = minimum(
            saturatingAdd(now_us, delay), slot.contract.expires_at_us);
        slot.next_attempt_us = retry_at;
        return retry_at;
    }
};

/* Fixed-storage receive-side ordering for links that may delay, duplicate, drop,
 * or reorder packets. The session id prevents delayed traffic from a previous
 * sender boot from entering the current stream. No work is performed in an ISR. */
template <size_t SLOTS, size_t BYTES>
class SequencedWirelessReceiver {
public:
    static_assert(SLOTS > 0, "sequenced receiver requires at least one slot");
    static_assert(BYTES > 0, "sequenced receiver requires nonzero payload capacity");

    SequencedWirelessReceiver(uint32_t session, uint32_t initial_sequence = 0)
        : session_(session), next_sequence_(initial_sequence) {
        clearSlots();
        memset(&diagnostics_, 0, sizeof(diagnostics_));
    }

    uint32_t session() const { return session_; }
    uint32_t nextSequence() const { return next_sequence_; }
    const nobro_wireless_ingress_diagnostics_t &diagnostics() const {
        return diagnostics_;
    }

    void resetSession(uint32_t session, uint32_t initial_sequence = 0) {
        session_ = session;
        next_sequence_ = initial_sequence;
        clearSlots();
    }

    nobro_wireless_ingress_event_t ingest(
        uint64_t now_us,
        uint32_t session,
        uint32_t sequence,
        uint64_t expires_at_us,
        const uint8_t *payload,
        size_t length,
        uint8_t *destination,
        size_t capacity) {
        if (length > BYTES || length > UINT16_MAX ||
            (payload == NULL && length != 0)) {
            return event(NOBRO_WIRELESS_INGRESS_INVALID, sequence, 0, 0, false);
        }
        increment(diagnostics_.observed_packets);
        if (session != session_) {
            increment(diagnostics_.session_rejections);
            return event(NOBRO_WIRELESS_INGRESS_WRONG_SESSION, sequence, 0, 0, false);
        }
        if (now_us > expires_at_us) {
            increment(diagnostics_.expired_packets);
            if (sequence == next_sequence_) ++next_sequence_;
            return event(NOBRO_WIRELESS_INGRESS_EXPIRED, sequence, 0, 0, false);
        }

        const uint32_t distance = sequence - next_sequence_;
        if (distance == 0) {
            if (capacity < length || (destination == NULL && length != 0)) {
                return event(NOBRO_WIRELESS_INGRESS_INVALID, sequence, 0, 0, false);
            }
            if (length != 0) memcpy(destination, payload, length);
            ++next_sequence_;
            return delivered(sequence, length, false, 0);
        }
        if (distance >= UINT32_C(0x80000000)) {
            increment(diagnostics_.duplicate_packets);
            return event(NOBRO_WIRELESS_INGRESS_DUPLICATE, sequence, 0, 0, false);
        }
        if (distance > SLOTS) {
            if (capacity < length || (destination == NULL && length != 0)) {
                return event(NOBRO_WIRELESS_INGRESS_INVALID, sequence, 0, 0, false);
            }
            uint32_t buffered_before = 0;
            for (size_t i = 0; i < SLOTS; ++i) {
                if (slots_[i].occupied && (slots_[i].sequence - next_sequence_) < distance) {
                    ++buffered_before;
                }
            }
            const uint32_t lost = distance - buffered_before;
            add(diagnostics_.inferred_lost_packets, lost);
            add(diagnostics_.window_evictions, buffered_before);
            clearSlots();
            next_sequence_ = sequence + 1;
            if (length != 0) memcpy(destination, payload, length);
            return delivered(sequence, length, false, lost);
        }
        for (size_t i = 0; i < SLOTS; ++i) {
            if (slots_[i].occupied && slots_[i].sequence == sequence) {
                increment(diagnostics_.duplicate_packets);
                return event(NOBRO_WIRELESS_INGRESS_DUPLICATE, sequence, 0, 0, false);
            }
        }
        for (size_t i = 0; i < SLOTS; ++i) {
            if (!slots_[i].occupied) {
                if (length != 0) memcpy(slots_[i].payload, payload, length);
                slots_[i].sequence = sequence;
                slots_[i].expires_at_us = expires_at_us;
                slots_[i].length = static_cast<uint16_t>(length);
                slots_[i].occupied = true;
                increment(diagnostics_.buffered_packets);
                return event(NOBRO_WIRELESS_INGRESS_BUFFERED, sequence, 0, 0, false);
            }
        }
        increment(diagnostics_.backpressure_rejections);
        return event(NOBRO_WIRELESS_INGRESS_BACKPRESSURE, sequence, 0, 0, false);
    }

    nobro_wireless_ingress_event_t drain(
        uint64_t now_us, uint8_t *destination, size_t capacity) {
        uint32_t lost_before = 0;
        size_t index = find(next_sequence_);
        if (index == SLOTS) {
            uint32_t nearest = UINT32_MAX;
            for (size_t i = 0; i < SLOTS; ++i) {
                if (!slots_[i].occupied || now_us <= slots_[i].expires_at_us) continue;
                const uint32_t distance = slots_[i].sequence - next_sequence_;
                if (distance < UINT32_C(0x80000000) && distance < nearest) {
                    nearest = distance;
                    index = i;
                }
            }
            if (index != SLOTS) {
                lost_before = nearest;
                add(diagnostics_.inferred_lost_packets, nearest);
                next_sequence_ = slots_[index].sequence;
            }
        }
        if (index == SLOTS) {
            return event(NOBRO_WIRELESS_INGRESS_EMPTY, 0, 0, 0, false);
        }
        Slot &slot = slots_[index];
        const uint32_t sequence = slot.sequence;
        if (now_us > slot.expires_at_us) {
            slot = emptySlot();
            ++next_sequence_;
            increment(diagnostics_.expired_packets);
            return event(
                NOBRO_WIRELESS_INGRESS_EXPIRED, sequence, 0, lost_before, false);
        }
        if (capacity < slot.length || (destination == NULL && slot.length != 0)) {
            return event(NOBRO_WIRELESS_INGRESS_INVALID, sequence, 0, 0, false);
        }
        if (slot.length != 0) memcpy(destination, slot.payload, slot.length);
        const size_t length = slot.length;
        slot = emptySlot();
        ++next_sequence_;
        return delivered(sequence, length, true, lost_before);
    }

private:
    struct Slot {
        uint8_t payload[BYTES];
        uint64_t expires_at_us;
        uint32_t sequence;
        uint16_t length;
        bool occupied;
    };

    Slot slots_[SLOTS];
    uint32_t session_;
    uint32_t next_sequence_;
    nobro_wireless_ingress_diagnostics_t diagnostics_;

    static Slot emptySlot() {
        Slot slot;
        memset(&slot, 0, sizeof(slot));
        return slot;
    }

    void clearSlots() { memset(slots_, 0, sizeof(slots_)); }

    size_t find(uint32_t sequence) const {
        for (size_t i = 0; i < SLOTS; ++i) {
            if (slots_[i].occupied && slots_[i].sequence == sequence) return i;
        }
        return SLOTS;
    }

    static void increment(uint32_t &value) {
        if (value != UINT32_MAX) ++value;
    }

    static void add(uint32_t &value, uint32_t amount) {
        value = UINT32_MAX - value < amount ? UINT32_MAX : value + amount;
    }

    static nobro_wireless_ingress_event_t event(
        nobro_wireless_ingress_event_kind_t kind,
        uint32_t sequence,
        uint16_t bytes,
        uint32_t lost_before,
        bool reordered) {
        nobro_wireless_ingress_event_t result = {
            kind, sequence, bytes, lost_before, reordered
        };
        return result;
    }

    nobro_wireless_ingress_event_t delivered(
        uint32_t sequence, size_t length, bool reordered, uint32_t lost_before) {
        increment(diagnostics_.delivered_packets);
        diagnostics_.delivered_bytes = UINT64_MAX - diagnostics_.delivered_bytes < length
            ? UINT64_MAX : diagnostics_.delivered_bytes + length;
        if (reordered) increment(diagnostics_.reordered_packets);
        return event(
            NOBRO_WIRELESS_INGRESS_DELIVERED,
            sequence,
            static_cast<uint16_t>(length),
            lost_before,
            reordered);
    }
};

/* Attempts to bring the physical link up (e.g. WiFi association). Returns true
 * when the link is now up. Called by ManagedWirelessLink::poll() only when the
 * recovery backoff says an attempt is due, so it is never busy-retried. */
typedef bool (*WirelessConnect)(void *context);

enum ManagedLinkState { MANAGED_LINK_DOWN = 0, MANAGED_LINK_UP = 1 };

struct ManagedLinkEvent {
    ManagedLinkState state;      /* link state after this poll */
    WirelessEvent queue_event;   /* queue outcome when serviced (up), else empty */
    bool reconnected;            /* this poll brought the link down->up */
    bool link_lost;              /* this poll took the link up->down */
    uint64_t next_action_us;     /* earliest time the next poll can make progress */
};

/* Cohesive managed WiFi/radio link. It unifies association recovery, bounded
 * adaptive messaging, and link-state gating so an application drives the whole
 * lifecycle with one poll() and two callbacks (connect + transport) instead of
 * hand-gluing WirelessRecovery, AdaptiveWirelessQueue, and the transport's
 * LINK_DOWN result. No allocation; the transport and connect callbacks are
 * called only from poll(), never from enqueue() or an ISR. When the transport
 * reports LINK_DOWN the link drops to DOWN and drives reconnect on the recovery
 * backoff; queued messages wait (or expire by their own deadline) meanwhile. */
template <size_t SLOTS, size_t BYTES>
class ManagedWirelessLink {
public:
    ManagedWirelessLink(const WirelessPolicy &policy, const WirelessRecovery &recovery)
        : queue_(policy), recovery_(recovery), state_(MANAGED_LINK_DOWN),
          reconnects_(0), link_losses_(0), connect_attempts_(0) {}

    bool valid() const { return queue_.valid() && recovery_.valid(); }
    bool up() const { return state_ == MANAGED_LINK_UP; }
    ManagedLinkState state() const { return state_; }
    uint32_t reconnects() const { return reconnects_; }
    uint32_t linkLosses() const { return link_losses_; }
    uint32_t connectAttempts() const { return connect_attempts_; }
    size_t queued() const { return queue_.size(); }
    const nobro_wireless_adaptive_diagnostics_t &diagnostics() const {
        return queue_.diagnostics();
    }

    /* Force the link down and rearm the recovery backoff (e.g. after the
     * application observes a fault the transport did not surface). */
    void reset() {
        state_ = MANAGED_LINK_DOWN;
        recovery_.reset();
    }

    /* Message admission delegates to the bounded queue; accepted while down too,
     * where a message waits until the link returns or its deadline elapses. */
    WirelessTicket enqueue(const uint8_t *payload, size_t length, const WirelessMessage &message) {
        return queue_.enqueue(payload, length, message);
    }
    WirelessTicket enqueueUrgentWithin(
        const uint8_t *payload, size_t length, uint64_t now_us, uint64_t within_us) {
        return queue_.enqueueUrgentWithin(payload, length, now_us, within_us);
    }
    WirelessTicket enqueueBestEffortFor(
        const uint8_t *payload, size_t length, uint64_t now_us, uint64_t useful_for_us) {
        return queue_.enqueueBestEffortFor(payload, length, now_us, useful_for_us);
    }
    bool cancel(WirelessTicket ticket) { return queue_.cancel(ticket); }

    ManagedLinkEvent poll(
        uint64_t now_us, WirelessConnect connect, WirelessSend send, void *context = NULL) {
        ManagedLinkEvent ev;
        ev.reconnected = false;
        ev.link_lost = false;
        ev.queue_event = emptyEvent(now_us);
        if (!valid() || connect == NULL || send == NULL) {
            ev.state = state_;
            ev.next_action_us = now_us;
            return ev;
        }
        if (state_ == MANAGED_LINK_DOWN) {
            if (recovery_.ready(now_us)) {
                ++connect_attempts_;
                if (connect(context)) {
                    state_ = MANAGED_LINK_UP;
                    recovery_.reset();
                    ++reconnects_;
                    ev.reconnected = true;
                    ev.state = MANAGED_LINK_UP;
                    ev.next_action_us = queue_.nextServiceUs();
                    return ev;
                }
                recovery_.failed(now_us); /* advance attempt count / backoff */
            }
            ev.state = MANAGED_LINK_DOWN;
            ev.next_action_us = recovery_.nextAttemptUs();
            return ev;
        }
        const WirelessEvent qe = queue_.service(now_us, send, context);
        ev.queue_event = qe;
        if (qe.kind == WIRELESS_RETRY_AT && qe.result == NOBRO_WIRELESS_SEND_LINK_DOWN) {
            state_ = MANAGED_LINK_DOWN;
            recovery_.reset();
            ++link_losses_;
            ev.link_lost = true;
            ev.state = MANAGED_LINK_DOWN;
            ev.next_action_us = now_us; /* reconnect attempt is due immediately */
            return ev;
        }
        ev.state = MANAGED_LINK_UP;
        ev.next_action_us = queue_.nextServiceUs();
        return ev;
    }

private:
    static WirelessEvent emptyEvent(uint64_t now_us) {
        WirelessEvent e = {WIRELESS_EMPTY, 0, now_us, NOBRO_WIRELESS_SEND_OK};
        return e;
    }

    AdaptiveWirelessQueue<SLOTS, BYTES> queue_;
    WirelessRecovery recovery_;
    ManagedLinkState state_;
    uint32_t reconnects_;
    uint32_t link_losses_;
    uint32_t connect_attempts_;
};

} // namespace nobro

#endif
