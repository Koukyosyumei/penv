#include <stdint.h>

#define REG(address) (*(volatile uint32_t *)(address))
#define TWI0 0x40003000u
#define TWI1 0x40004000u
#define UART0 0x40002000u

static volatile uint32_t irq_seen;

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
        report("TWI FAIL\n", 9); \
        __asm volatile("bkpt 0"); \
    } \
} while (0)

static void twi0_irq(void)
{
    if (REG(TWI0 + 0x124) == 1)
        irq_seen |= 1;
    REG(TWI0 + 0x124) = 0;
}

static void twi1_irq(void)
{
    if (REG(TWI1 + 0x11c) == 1)
        irq_seen |= 2;
    REG(TWI1 + 0x11c) = 0;
}

void reset(void)
{
    irq_seen = 0;
    REG(0xe000e100u) = (1u << 3) | (1u << 4);
    REG(TWI0 + 0x500) = 5;
    REG(TWI0 + 0x304) = 1u << 9;
    REG(TWI0 + 0x588) = 0x51;
    REG(TWI0 + 0x008) = 1;
    CHECK(irq_seen == 1 && REG(TWI0 + 0x4c4) == 2);
    REG(TWI0 + 0x4c4) = 2;
    CHECK(REG(TWI0 + 0x4c4) == 0);

    REG(TWI1 + 0x500) = 5;
    REG(TWI1 + 0x588) = 0x50;
    REG(TWI1 + 0x304) = 1u << 7;
    REG(TWI1 + 0x008) = 1;
    REG(TWI1 + 0x51c) = 0x12;
    REG(TWI1 + 0x51c) = 0x5a;
    REG(TWI1 + 0x51c) = 0xa5;
    REG(TWI1 + 0x014) = 1;
    CHECK(irq_seen == 3 && REG(TWI1 + 0x104) == 1);

    REG(TWI1 + 0x008) = 1;
    REG(TWI1 + 0x51c) = 0x12;
    REG(TWI1 + 0x01c) = 1;
    CHECK(REG(TWI1 + 0x148) == 1);
    REG(TWI1 + 0x200) = 1;
    REG(TWI1 + 0x000) = 1;
    CHECK(REG(TWI1 + 0x108) == 1 && REG(TWI1 + 0x518) == 0x5a);
    REG(TWI1 + 0x108) = 0;
    REG(TWI1 + 0x104) = 0;
    REG(TWI1 + 0x200) = 2;
    REG(TWI1 + 0x020) = 1;
    CHECK(REG(TWI1 + 0x108) == 1 && REG(TWI1 + 0x518) == 0xa5);
    CHECK(REG(TWI1 + 0x104) == 1);

    report("TWI OK\n", 7);
    __asm volatile("bkpt 0");
}

__attribute__((section(".vectors"), used))
const uintptr_t vectors[21] = {
    [0] = 0x20040000u,
    [1] = (uintptr_t)reset,
    [19] = (uintptr_t)twi0_irq,
    [20] = (uintptr_t)twi1_irq,
};
