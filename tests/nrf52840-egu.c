#include <stdint.h>

#define REG(address) (*(volatile uint32_t *)(address))
#define EGU0 0x40014000u
#define EGU5 0x40019000u
#define PPI 0x4001f000u
#define UART0 0x40002000u

static volatile uint32_t irq_sources;

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
        report("EGU FAIL\n", 9); \
        __asm volatile("bkpt 0"); \
    } \
} while (0)

static void egu0_irq(void)
{
    if (REG(EGU0 + 0x108)) {
        REG(EGU0 + 0x108) = 0;
        irq_sources |= 1;
    }
}

static void egu5_irq(void)
{
    if (REG(EGU5 + 0x13c)) {
        REG(EGU5 + 0x13c) = 0;
        irq_sources |= 2;
    }
}

void reset(void)
{
    irq_sources = 0;
    REG(0xe000e100u) = (1u << 20) | (1u << 25);
    REG(PPI + 0x510) = EGU0 + 0x108;
    REG(PPI + 0x514) = EGU5 + 0x03c;
    REG(PPI + 0x504) = 1;
    REG(EGU5 + 0x300) = 1u << 15;
    REG(EGU0 + 0x008) = 0;
    CHECK(irq_sources == 0 && REG(EGU0 + 0x108) == 0 && REG(EGU5 + 0x13c) == 0);
    REG(EGU0 + 0x008) = 1;
    CHECK(irq_sources == 2 && REG(EGU0 + 0x108) == 1);
    REG(EGU0 + 0x304) = 1u << 2;
    CHECK(irq_sources == 3 && REG(EGU0 + 0x108) == 0 && REG(EGU5 + 0x13c) == 0);
    REG(EGU0 + 0x308) = 1u << 2;
    CHECK(REG(EGU0 + 0x300) == 0);
    report("EGU OK\n", 7);
    __asm volatile("bkpt 0");
}

__attribute__((section(".vectors"), used))
const uintptr_t vectors[42] = {
    [0] = 0x20040000u,
    [1] = (uintptr_t)reset,
    [16 + 20] = (uintptr_t)egu0_irq,
    [16 + 25] = (uintptr_t)egu5_irq,
};
