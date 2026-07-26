#ifndef NOBRO_ARDUINO_NATIVE_NETWORK_H
#define NOBRO_ARDUINO_NATIVE_NETWORK_H

#include "Arduino.h"
#include "nobro_wireless.h"

namespace nobro {

/*
 * Fixed-slot TCP/UDP data plane over a board package's native WiFi classes.
 *
 * Provider supplies TcpClient, UdpSocket, ready(), resolve(), backendId(), and
 * mtu(). The wrapper retains no host names, endpoints, or payloads. Vendor
 * client classes can still own heap/controller resources, which is exposed as
 * NOBRO_NETWORK_VENDOR_MANAGED and must be priced for the chosen board stack.
 * A zero buffer byte count means that the vendor does not expose the capacity;
 * it must never be interpreted as zero vendor allocation.
 */
template <typename Provider, size_t TCP_SOCKETS, size_t UDP_SOCKETS>
class ArduinoNativeNetwork {
public:
    ArduinoNativeNetwork()
        : state_(NOBRO_STACK_DOWN), lifecycle_generation_(0) {
        for (size_t index = 0; index < TCP_SOCKETS; ++index) {
            tcp_[index].open = false;
            tcp_[index].generation = 0;
        }
        for (size_t index = 0; index < UDP_SOCKETS; ++index) {
            udp_[index].open = false;
            udp_[index].generation = 0;
            udp_[index].remote_port = 0;
        }
    }

    ArduinoNativeNetwork(const ArduinoNativeNetwork &) = delete;
    ArduinoNativeNetwork &operator=(const ArduinoNativeNetwork &) = delete;

    nobro_network_capabilities_t capabilities() const {
        const nobro_network_capabilities_t value = {
            Provider::backendId(),
            static_cast<uint16_t>(TCP_SOCKETS),
            static_cast<uint16_t>(UDP_SOCKETS),
            Provider::mtu(),
            Provider::rxBufferBytes(),
            Provider::txBufferBytes(),
            NOBRO_NETWORK_VENDOR_MANAGED,
            true,
        };
        return value;
    }

    nobro_stack_result_t mount(uint16_t instance,
                               nobro_network_mount_receipt_t &receipt) {
        if (!validConfiguration()) {
            return NOBRO_STACK_INVALID_IDENTITY;
        }
        if (state_ != NOBRO_STACK_DOWN && state_ != NOBRO_STACK_QUIESCED) {
            return NOBRO_STACK_BUSY;
        }
        state_ = NOBRO_STACK_STARTING;
        if (!Provider::ready()) {
            state_ = NOBRO_STACK_DOWN;
            return NOBRO_STACK_NOT_READY;
        }
        lifecycle_generation_ =
            lifecycle_generation_ == UINT32_MAX ? 1U : lifecycle_generation_ + 1U;
        state_ = NOBRO_STACK_READY;
        receipt.instance = instance;
        receipt.capabilities = capabilities();
        receipt.lifecycle_generation = lifecycle_generation_;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t resolve(const uint8_t *host,
                                 size_t host_len,
                                 uint64_t now_us,
                                 uint64_t deadline_us,
                                 nobro_ipv4_address_t &address) {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        if (!validDeadline(now_us, deadline_us)) {
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        if (host == 0 || host_len == 0 || host_len > 253) {
            return NOBRO_STACK_INVALID_CONFIG;
        }
        char text[254] = {};
        for (size_t index = 0; index < host_len; ++index) {
            if (host[index] < 0x21 || host[index] > 0x7e) {
                return NOBRO_STACK_INVALID_CONFIG;
            }
            text[index] = static_cast<char>(host[index]);
        }
        IPAddress resolved;
        const uint32_t started = micros();
        const int result = Provider::resolve(text, resolved);
        if (deadlineMissed(started, now_us, deadline_us)) {
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        if (result != 1) {
            return NOBRO_STACK_BACKEND_FAULT;
        }
        copyAddress(resolved, address);
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t open(nobro_network_socket_kind_t kind,
                              uint16_t local_port,
                              nobro_network_socket_t &socket) {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        if (kind == NOBRO_NETWORK_TCP) {
            if (local_port != 0) {
                return NOBRO_STACK_INVALID_CONFIG;
            }
            for (size_t index = 0; index < TCP_SOCKETS; ++index) {
                if (!tcp_[index].open) {
                    tcp_[index].open = true;
                    tcp_[index].generation = next(tcp_[index].generation);
                    socket = makeSocket(kind, index, tcp_[index].generation);
                    return NOBRO_STACK_OK;
                }
            }
            return NOBRO_STACK_QUEUE_FULL;
        }
        if (kind == NOBRO_NETWORK_UDP) {
            if (local_port == 0) {
                return NOBRO_STACK_INVALID_CONFIG;
            }
            for (size_t index = 0; index < UDP_SOCKETS; ++index) {
                if (!udp_[index].open) {
                    if (udp_[index].client.begin(local_port) != 1) {
                        return NOBRO_STACK_BACKEND_FAULT;
                    }
                    udp_[index].open = true;
                    udp_[index].generation = next(udp_[index].generation);
                    socket = makeSocket(kind, index, udp_[index].generation);
                    return NOBRO_STACK_OK;
                }
            }
            return NOBRO_STACK_QUEUE_FULL;
        }
        return NOBRO_STACK_INVALID_CONFIG;
    }

    nobro_stack_result_t connect(nobro_network_socket_t socket,
                                 const nobro_network_endpoint_t &remote,
                                 uint64_t now_us,
                                 uint64_t deadline_us) {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        if (!validDeadline(now_us, deadline_us) || remote.port == 0) {
            return remote.port == 0 ? NOBRO_STACK_INVALID_CONFIG
                                    : NOBRO_STACK_DEADLINE_ELAPSED;
        }
        const IPAddress address = toAddress(remote.address);
        const uint32_t started = micros();
        int result = 1;
        if (socket.kind == NOBRO_NETWORK_TCP) {
            TcpSlot *slot = tcpSlot(socket);
            if (slot == 0) {
                return NOBRO_STACK_INVALID_CONFIG;
            }
            result = slot->client.connect(address, remote.port);
        } else if (socket.kind == NOBRO_NETWORK_UDP) {
            UdpSlot *slot = udpSlot(socket);
            if (slot == 0) {
                return NOBRO_STACK_INVALID_CONFIG;
            }
            slot->remote = address;
            slot->remote_port = remote.port;
        } else {
            return NOBRO_STACK_INVALID_CONFIG;
        }
        if (deadlineMissed(started, now_us, deadline_us)) {
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        return result == 1 ? NOBRO_STACK_OK : NOBRO_STACK_BACKEND_FAULT;
    }

    nobro_stack_result_t send(nobro_network_socket_t socket,
                              const uint8_t *payload,
                              size_t payload_len,
                              uint64_t now_us,
                              uint64_t deadline_us,
                              nobro_network_io_receipt_t &receipt) {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        if (payload == 0 || payload_len == 0 ||
            payload_len > Provider::mtu()) {
            return NOBRO_STACK_INVALID_CONFIG;
        }
        if (!validDeadline(now_us, deadline_us)) {
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        size_t written = 0;
        const uint32_t started = micros();
        if (socket.kind == NOBRO_NETWORK_TCP) {
            TcpSlot *slot = tcpSlot(socket);
            if (slot == 0 || slot->client.connected() == 0) {
                return NOBRO_STACK_NOT_READY;
            }
            written = slot->client.write(payload, payload_len);
        } else if (socket.kind == NOBRO_NETWORK_UDP) {
            UdpSlot *slot = udpSlot(socket);
            if (slot == 0 || slot->remote_port == 0) {
                return NOBRO_STACK_NOT_READY;
            }
            if (slot->client.beginPacket(slot->remote, slot->remote_port) != 1) {
                return NOBRO_STACK_BACKEND_FAULT;
            }
            written = slot->client.write(payload, payload_len);
            if (written != payload_len || slot->client.endPacket() != 1) {
                return NOBRO_STACK_BACKEND_FAULT;
            }
        } else {
            return NOBRO_STACK_INVALID_CONFIG;
        }
        if (deadlineMissed(started, now_us, deadline_us)) {
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        if (written != payload_len || written > UINT16_MAX) {
            return NOBRO_STACK_BACKEND_FAULT;
        }
        receipt.socket = socket;
        receipt.bytes = static_cast<uint16_t>(written);
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t receive(nobro_network_socket_t socket,
                                 uint8_t *output,
                                 size_t capacity,
                                 uint64_t now_us,
                                 uint64_t deadline_us,
                                 nobro_network_io_receipt_t &receipt) {
        if (state_ != NOBRO_STACK_READY) {
            return NOBRO_STACK_NOT_READY;
        }
        if (output == 0 || capacity == 0 || capacity > UINT16_MAX ||
            capacity > Provider::mtu()) {
            return NOBRO_STACK_INVALID_CONFIG;
        }
        if (!validDeadline(now_us, deadline_us)) {
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        int available = 0;
        int received = 0;
        const uint32_t started = micros();
        if (socket.kind == NOBRO_NETWORK_TCP) {
            TcpSlot *slot = tcpSlot(socket);
            if (slot == 0) {
                return NOBRO_STACK_INVALID_CONFIG;
            }
            available = slot->client.available();
            if (available < 0) {
                return NOBRO_STACK_BACKEND_FAULT;
            }
            const size_t amount = minSize(static_cast<size_t>(available), capacity);
            received = amount == 0 ? 0 : slot->client.read(output, amount);
        } else if (socket.kind == NOBRO_NETWORK_UDP) {
            UdpSlot *slot = udpSlot(socket);
            if (slot == 0) {
                return NOBRO_STACK_INVALID_CONFIG;
            }
            available = slot->client.parsePacket();
            if (available < 0) {
                return NOBRO_STACK_BACKEND_FAULT;
            }
            const size_t amount = minSize(static_cast<size_t>(available), capacity);
            received = amount == 0 ? 0 : slot->client.read(output, amount);
        } else {
            return NOBRO_STACK_INVALID_CONFIG;
        }
        if (deadlineMissed(started, now_us, deadline_us)) {
            return NOBRO_STACK_DEADLINE_ELAPSED;
        }
        if (received < 0 || static_cast<size_t>(received) > capacity) {
            return NOBRO_STACK_BACKEND_FAULT;
        }
        receipt.socket = socket;
        receipt.bytes = static_cast<uint16_t>(received);
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t close(nobro_network_socket_t socket) {
        if (socket.kind == NOBRO_NETWORK_TCP) {
            TcpSlot *slot = tcpSlot(socket);
            if (slot == 0) {
                return NOBRO_STACK_INVALID_CONFIG;
            }
            slot->client.stop();
            slot->open = false;
            return NOBRO_STACK_OK;
        }
        if (socket.kind == NOBRO_NETWORK_UDP) {
            UdpSlot *slot = udpSlot(socket);
            if (slot == 0) {
                return NOBRO_STACK_INVALID_CONFIG;
            }
            slot->client.stop();
            slot->open = false;
            slot->remote_port = 0;
            return NOBRO_STACK_OK;
        }
        return NOBRO_STACK_INVALID_CONFIG;
    }

    nobro_stack_result_t quiesce() {
        closeAll();
        state_ = NOBRO_STACK_QUIESCED;
        return NOBRO_STACK_OK;
    }

    nobro_stack_result_t recover() {
        closeAll();
        state_ = Provider::ready() ? NOBRO_STACK_READY : NOBRO_STACK_DOWN;
        return state_ == NOBRO_STACK_READY ? NOBRO_STACK_OK
                                           : NOBRO_STACK_NOT_READY;
    }

    nobro_stack_state_t state() const { return state_; }
    size_t staticRamBytes() const { return sizeof(*this); }

private:
    struct TcpSlot {
        typename Provider::TcpClient client;
        uint16_t generation;
        bool open;
    };

    struct UdpSlot {
        typename Provider::UdpSocket client;
        IPAddress remote;
        uint16_t remote_port;
        uint16_t generation;
        bool open;
    };

    static bool validConfiguration() {
        return TCP_SOCKETS <= UINT16_MAX && UDP_SOCKETS <= UINT16_MAX &&
               (TCP_SOCKETS != 0 || UDP_SOCKETS != 0) && Provider::mtu() != 0;
    }

    static uint16_t next(uint16_t generation) {
        return generation == UINT16_MAX ? 1U
                                        : static_cast<uint16_t>(generation + 1U);
    }

    static nobro_network_socket_t makeSocket(nobro_network_socket_kind_t kind,
                                             size_t slot,
                                             uint16_t generation) {
        const nobro_network_socket_t socket = {
            kind,
            static_cast<uint16_t>(slot),
            generation,
        };
        return socket;
    }

    static bool validDeadline(uint64_t now_us, uint64_t deadline_us) {
        return deadline_us > now_us;
    }

    static bool deadlineMissed(uint32_t started,
                               uint64_t now_us,
                               uint64_t deadline_us) {
        return static_cast<uint64_t>(
                   static_cast<uint32_t>(micros() - started)) >
               deadline_us - now_us;
    }

    static size_t minSize(size_t left, size_t right) {
        return left < right ? left : right;
    }

    static IPAddress toAddress(const nobro_ipv4_address_t &address) {
        return IPAddress(address.octets[0], address.octets[1],
                         address.octets[2], address.octets[3]);
    }

    static void copyAddress(const IPAddress &source,
                            nobro_ipv4_address_t &destination) {
        for (size_t index = 0; index < 4; ++index) {
            destination.octets[index] = source[index];
        }
    }

    TcpSlot *tcpSlot(nobro_network_socket_t socket) {
        if (socket.slot >= TCP_SOCKETS) {
            return 0;
        }
        TcpSlot &slot = tcp_[socket.slot];
        return slot.open && slot.generation == socket.generation ? &slot : 0;
    }

    UdpSlot *udpSlot(nobro_network_socket_t socket) {
        if (socket.slot >= UDP_SOCKETS) {
            return 0;
        }
        UdpSlot &slot = udp_[socket.slot];
        return slot.open && slot.generation == socket.generation ? &slot : 0;
    }

    void closeAll() {
        for (size_t index = 0; index < TCP_SOCKETS; ++index) {
            if (tcp_[index].open) {
                tcp_[index].client.stop();
                tcp_[index].open = false;
            }
        }
        for (size_t index = 0; index < UDP_SOCKETS; ++index) {
            if (udp_[index].open) {
                udp_[index].client.stop();
                udp_[index].open = false;
                udp_[index].remote_port = 0;
            }
        }
    }

    TcpSlot tcp_[TCP_SOCKETS];
    UdpSlot udp_[UDP_SOCKETS];
    nobro_stack_state_t state_;
    uint32_t lifecycle_generation_;
};

}  // namespace nobro

#endif
