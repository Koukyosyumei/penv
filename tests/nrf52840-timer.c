#include <stdint.h>

#define REG(address) (*(volatile uint32_t *)(address))
#define TIMER0 0x40008000u
#define TIMER3 0x4001a000u
#define PPI 0x4001f000u
#define UART0 0x40002000u

static volatile uint32_t timer0_hits;
static volatile uint32_t timer3_hits;

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
        report("TIMER FAIL\n", 11); \
        __asm volatile("bkpt 0"); \
    } \
} while (0)

static void timer0_irq(void)
{
    if (REG(TIMER0 + 0x140) == 1) {
        REG(TIMER0 + 0x140) = 0;
        ++timer0_hits;
    }
}

static void timer3_irq(void)
{
    if (REG(TIMER3 + 0x154) == 1) {
        REG(TIMER3 + 0x154) = 0;
        ++timer3_hits;
    }
}

void reset(void)
{
    timer0_hits = 0;
    timer3_hits = 0;
    REG(0xe000e100u) = (1u << 8) | (1u << 26);

    REG(TIMER3 + 0x504) = 1;
    REG(TIMER3 + 0x554) = 1;
    REG(TIMER3 + 0x304) = 1u << 21;
    REG(TIMER3 + 0x000) = 1;

    REG(PPI + 0x510) = TIMER0 + 0x140;
    REG(PPI + 0x514) = TIMER3 + 0x008;
    REG(PPI + 0x504) = 1;

    CHECK(REG(TIMER0 + 0x510) == 4);
    REG(TIMER0 + 0x508) = 3;
    REG(TIMER0 + 0x510) = 0;
    REG(TIMER0 + 0x540) = 200;
    REG(TIMER0 + 0x200) = 1u << 8;
    REG(TIMER0 + 0x304) = 1u << 16;
    REG(TIMER0 + 0x000) = 1;

    while (timer0_hits == 0 || timer3_hits == 0)
        ;

    REG(TIMER0 + 0x044) = 1;
    REG(TIMER3 + 0x040) = 1;
    CHECK(timer0_hits == 1 && timer3_hits == 1);
    CHECK(REG(TIMER0 + 0x544) == 200 && REG(TIMER3 + 0x540) == 1);
    CHECK(REG(TIMER0 + 0x140) == 0 && REG(TIMER3 + 0x154) == 0);

    report("TIMER OK\n", 9);
    __asm volatile("bkpt 0");
}

__attribute__((section(".vectors"), used))
const uintptr_t vectors[43] = {
    [0] = 0x20040000u,
    [1] = (uintptr_t)reset,
    [16 + 8] = (uintptr_t)timer0_irq,
    [16 + 26] = (uintptr_t)timer3_irq,
};
