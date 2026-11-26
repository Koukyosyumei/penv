#include <stdint.h>

#define REG(address) (*(volatile uint32_t *)(address))
#define RNG 0x4000d000u
#define UART0 0x40002000u

static volatile uint32_t hits;
static volatile uint32_t values[3];

static void report(const char *text, unsigned length)
{
    volatile char *buffer = (volatile char *)0x20000000u;
    for (unsigned index = 0; index < length; ++index)
        buffer[index] = text[index];
    REG(UART0 + 0x500) = 8;
    REG(UART0 + 0x544) = (uintptr_t)buffer;
    REG(UART0 + 0x548) = length;
    REG(UART0 + 0x008) = 1;
}

#define CHECK(condition) do { \
    if (!(condition)) { \
        report("RNG FAIL\n", 9); \
        __asm volatile("bkpt 0"); \
    } \
} while (0)

static void wait_past_period(void)
{
    for (unsigned i = 0; i < 13000; ++i)
        __asm volatile("nop");
}

static void rng_irq(void)
{
    if (REG(RNG + 0x100)) {
        REG(RNG + 0x100) = 0;
        if (hits < 3)
            values[hits] = REG(RNG + 0x508);
        if (++hits == 2)
            REG(RNG + 0x004) = 1;
    }
}

void reset(void)
{
    hits = 0;
    REG(0xe000e100u) = 1u << 13;
    REG(RNG + 0x304) = 1;
    REG(RNG) = 1;
    REG(RNG + 4) = 1;
    wait_past_period();
    CHECK(hits == 0 && REG(RNG + 0x100) == 0);

    REG(RNG) = 1;
    while (hits < 2)
        ;
    wait_past_period();
    CHECK(hits == 2 && values[0] <= 255 && values[1] <= 255 && values[0] != values[1]);
    CHECK(REG(RNG + 0x508) == values[1]);

    REG(RNG + 0x200) = 1;
    REG(RNG + 0x504) = 1;
    REG(RNG + 0x308) = 1;
    REG(RNG) = 1;
    while (!REG(RNG + 0x100))
        ;
    CHECK(hits == 2);
    REG(RNG + 0x304) = 1;
    CHECK(hits == 3 && values[2] <= 255);
    wait_past_period();
    CHECK(hits == 3 && REG(RNG + 0x100) == 0);

    report("RNG OK\n", 7);
    __asm volatile("bkpt 0");
}

__attribute__((section(".vectors"), used))
const uintptr_t vectors[30] = {
    [0] = 0x20040000u,
    [1] = (uintptr_t)reset,
    [16 + 13] = (uintptr_t)rng_irq,
};
