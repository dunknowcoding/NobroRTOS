/* Candidate-only kernel-lite compile witness.
 *
 * This file proves that the bounded arithmetic/state model can be represented by
 * an exact target compiler. It is not a board profile, HAL, scheduler, startup,
 * interrupt, timing, or physical-support claim. */
#include <limits.h>
#include <stdint.h>

#ifndef NOBRO_TARGET_ID
#error "NOBRO_TARGET_ID must name the exact candidate compile contract"
#endif

typedef char nobro_u16_is_16_bits[(sizeof(uint16_t) * CHAR_BIT == 16) ? 1 : -1];
typedef char nobro_u32_is_32_bits[(sizeof(uint32_t) * CHAR_BIT == 32) ? 1 : -1];

struct nobro_task_lite {
    uint16_t period;
    uint16_t budget;
};

struct nobro_mailbox_lite {
    uint16_t slots[4];
    uint16_t head;
    uint16_t length;
};

volatile uint16_t nobro_kernel_lite_result;
volatile uint16_t nobro_target_storage_unit_bits = CHAR_BIT;

static uint16_t deadline_reached(uint16_t now, uint16_t deadline)
{
    return (uint16_t)(now - deadline) < UINT16_C(0x8000);
}

static uint16_t admit(const struct nobro_task_lite *tasks, uint16_t count)
{
    uint16_t index;
    if (tasks == 0 || count == 0 || count > 4) return 0;
    for (index = 0; index < count; ++index) {
        if (tasks[index].period == 0 || tasks[index].budget == 0 ||
                tasks[index].budget > tasks[index].period)
            return 0;
    }
    return 1;
}

static void push_drop_oldest(struct nobro_mailbox_lite *box, uint16_t value)
{
    uint16_t tail;
    if (box->length == 4) {
        box->head = (uint16_t)((box->head + 1) & 3);
        --box->length;
    }
    tail = (uint16_t)((box->head + box->length) & 3);
    box->slots[tail] = value;
    ++box->length;
}

static uint16_t crc8_07(const char *bytes, uint16_t length)
{
    uint16_t crc = 0;
    uint16_t index;
    for (index = 0; index < length; ++index) {
        uint16_t bit;
        crc = (uint16_t)((crc ^ ((uint16_t)bytes[index] & 0xffu)) & 0xffu);
        for (bit = 0; bit < 8; ++bit)
            crc = (uint16_t)(((crc & 0x80u) != 0
                ? (crc << 1) ^ 0x07u : crc << 1) & 0xffu);
    }
    return crc;
}

static uint16_t selftest(void)
{
    static const struct nobro_task_lite good[2] = {{100, 20}, {1000, 40}};
    static const struct nobro_task_lite bad[1] = {{10, 11}};
    struct nobro_mailbox_lite box;
    uint16_t value;
    box.head = 0;
    box.length = 0;
    for (value = 0; value < 6; ++value) push_drop_oldest(&box, value);
    return (uint16_t)(admit(good, 2) && !admit(bad, 1) &&
        box.length == 4 && box.slots[box.head] == 2 &&
        deadline_reached(4, UINT16_C(65530)) &&
        !deadline_reached(UINT16_C(65530), 4) &&
        crc8_07("123456789", 9) == 0xf4u);
}

int main(void)
{
    nobro_kernel_lite_result = selftest();
    for (;;) {
        /* The exact board/HAL owns sleep, watchdog, interrupts and reporting. */
    }
}
