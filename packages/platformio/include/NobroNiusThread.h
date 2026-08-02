#ifndef NOBRO_NIUS_THREAD_H
#define NOBRO_NIUS_THREAD_H

#include <Thread.h>
#include "nobro_mesh.h"

namespace nobro {

template <size_t MaxTextBytes = 120, size_t MaxPathBytes = 32,
          size_t MaxAddressBytes = 48>
class NiusThreadAdapter {
public:
    explicit NiusThreadAdapter(ThreadClass &thread)
        : thread_(thread), channel_(0), pan_id_(0), mode_(ThreadClass::LINK_ROUTER),
          poll_ms_(0), coap_port_(0), state_(NOBRO_MESH_DOWN), diagnostics_{} {
        network_name_[0] = '\0';
        for (size_t i = 0; i < sizeof network_key_; ++i) network_key_[i] = 0;
    }

    nobro_mesh_result_t begin(const char *network_name, uint8_t channel,
                              uint16_t pan_id, const uint8_t network_key[16],
                              ThreadClass::LinkMode mode = ThreadClass::LINK_ROUTER,
                              uint32_t poll_ms = 1000) {
        if (state_ != NOBRO_MESH_DOWN || network_key == nullptr || channel < 11 ||
            channel > 26 || (mode == ThreadClass::LINK_SED && poll_ms == 0)) {
            return reject(NOBRO_MESH_INVALID_CONFIG);
        }
        const size_t name_bytes = boundedLength(network_name, 17);
        if (name_bytes == 0 || name_bytes > 16) return reject(NOBRO_MESH_INVALID_CONFIG);
        for (size_t i = 0; i <= name_bytes; ++i) network_name_[i] = network_name[i];
        for (size_t i = 0; i < 16; ++i) network_key_[i] = network_key[i];
        channel_ = channel;
        pan_id_ = pan_id;
        mode_ = mode;
        poll_ms_ = poll_ms;
        return startStack(false);
    }

    nobro_mesh_result_t process(uint32_t deadline_us) {
        if (state_ != NOBRO_MESH_ATTACHING && state_ != NOBRO_MESH_ATTACHED) {
            return reject(NOBRO_MESH_NOT_READY);
        }
        thread_.process();
        increment(diagnostics_.process_calls);
        state_ = thread_.isAttached() ? NOBRO_MESH_ATTACHED : NOBRO_MESH_ATTACHING;
        if (static_cast<int32_t>(micros() - deadline_us) > 0) {
            increment(diagnostics_.deadline_misses);
            return reject(NOBRO_MESH_DEADLINE_MISS);
        }
        return NOBRO_MESH_OK;
    }

    nobro_mesh_result_t coapStart(uint16_t port = 5683) {
        if (state_ != NOBRO_MESH_ATTACHED || port == 0) {
            return reject(NOBRO_MESH_NOT_READY);
        }
        if (thread_.coapStart(port) != ThreadClass::THREAD_OK) {
            return reject(NOBRO_MESH_PROVIDER_ERROR);
        }
        coap_port_ = port;
        return NOBRO_MESH_OK;
    }

    nobro_mesh_result_t coapPost(const char *address, const char *path,
                                 const char *text) {
        if (state_ != NOBRO_MESH_ATTACHED || coap_port_ == 0) {
            return reject(NOBRO_MESH_NOT_READY);
        }
        if (!validBounded(address, MaxAddressBytes) || !validBounded(path, MaxPathBytes) ||
            boundedLength(text, MaxTextBytes + 1) > MaxTextBytes) {
            return reject(NOBRO_MESH_TOO_LARGE);
        }
        if (thread_.coapPost(address, path, text) != ThreadClass::THREAD_OK) {
            return reject(NOBRO_MESH_PROVIDER_ERROR);
        }
        increment(diagnostics_.messages_accepted);
        return NOBRO_MESH_OK;
    }

    void quiesce() {
        if (state_ == NOBRO_MESH_DOWN || state_ == NOBRO_MESH_SUSPENDED) return;
        thread_.stop();
        thread_.end();
        state_ = NOBRO_MESH_SUSPENDED;
        coap_port_ = 0;
    }

    nobro_mesh_result_t recover() {
        if (state_ != NOBRO_MESH_SUSPENDED) return reject(NOBRO_MESH_NOT_READY);
        return startStack(true);
    }

    void release() {
        if (state_ != NOBRO_MESH_DOWN) {
            thread_.stop();
            thread_.end();
        }
        state_ = NOBRO_MESH_DOWN;
        coap_port_ = 0;
    }

    nobro_mesh_state_t state() const { return state_; }
    ThreadClass::Role role() const { return thread_.role(); }
    uint16_t rloc16() const { return thread_.rloc16(); }
    nobro_mesh_diagnostics_t diagnostics() const { return diagnostics_; }

private:
    nobro_mesh_result_t startStack(bool recovery) {
        if (thread_.begin() != ThreadClass::THREAD_OK ||
            thread_.setNetwork(network_name_, channel_, pan_id_, network_key_) !=
                ThreadClass::THREAD_OK ||
            thread_.setLinkMode(mode_, poll_ms_) != ThreadClass::THREAD_OK ||
            thread_.start() != ThreadClass::THREAD_OK) {
            thread_.end();
            state_ = NOBRO_MESH_FAULTED;
            return reject(NOBRO_MESH_PROVIDER_ERROR);
        }
        state_ = thread_.isAttached() ? NOBRO_MESH_ATTACHED : NOBRO_MESH_ATTACHING;
        if (recovery) increment(diagnostics_.recoveries);
        return NOBRO_MESH_OK;
    }

    static size_t boundedLength(const char *text, size_t limit) {
        if (text == nullptr) return limit;
        size_t length = 0;
        while (length < limit && text[length] != '\0') ++length;
        return length;
    }

    static bool validBounded(const char *text, size_t capacity) {
        const size_t length = boundedLength(text, capacity + 1);
        return length > 0 && length <= capacity;
    }

    nobro_mesh_result_t reject(nobro_mesh_result_t result) {
        increment(diagnostics_.rejected);
        return result;
    }

    static void increment(uint32_t &value) {
        if (value != UINT32_MAX) ++value;
    }

    ThreadClass &thread_;
    char network_name_[17];
    uint8_t network_key_[16];
    uint8_t channel_;
    uint16_t pan_id_;
    ThreadClass::LinkMode mode_;
    uint32_t poll_ms_;
    uint16_t coap_port_;
    nobro_mesh_state_t state_;
    nobro_mesh_diagnostics_t diagnostics_;
};

}  // namespace nobro

#endif
