#include <stdint.h>

extern int main(void);
extern uint32_t _stack_top;

void reset_handler(void)
{
    (void)main();
    for (;;) {}
}

__attribute__((used, section(".isr_vector")))
void (*const vectors[16])(void) = {
    (void (*)(void))(&_stack_top), reset_handler,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
};
