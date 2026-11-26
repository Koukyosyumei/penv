#include <stdint.h>

#define REG(address) (*(volatile uint32_t *)(address))
#define I2S 0x40025000u
#define UART0 0x40002000u
#define TX0 0x20000020u
#define RX0 0x20000040u
#define TX1 0x20000060u
#define RX1 0x20000080u

static volatile uint32_t updates;
static volatile uint32_t stopped;

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
        report("I2S FAIL\n", 9); \
        __asm volatile("bkpt 0"); \
    } \
} while (0)

static void i2s_irq(void)
{
    if (REG(I2S + 0x104) && REG(I2S + 0x114)) {
        REG(I2S + 0x104) = 0;
        REG(I2S + 0x114) = 0;
        ++updates;
        if (updates == 1) {
            REG(I2S + 0x538) = RX1;
            REG(I2S + 0x540) = TX1;
        } else if (updates == 3) {
            REG(I2S + 0x004) = 1;
        }
    }
    if (REG(I2S + 0x108)) {
        REG(I2S + 0x108) = 0;
        ++stopped;
    }
}

void reset(void)
{
    updates = 0;
    stopped = 0;
    REG(TX0) = 0x1234abcd;
    REG(TX0 + 4) = 0xff00bc9a;
    REG(TX1) = 0x76543210;
    REG(TX1 + 4) = 0x11223344;
    REG(RX0 + 8) = 0xaa5555aa;
    REG(RX1 + 8) = 0x55aaaa55;

    REG(0xe000e104u) = 1u << (37 - 32);
    REG(I2S + 0x000) = 1;
    CHECK(REG(I2S + 0x104) == 0 && REG(I2S + 0x114) == 0);
    REG(I2S + 0x508) = 1;
    REG(I2S + 0x538) = RX0;
    REG(I2S + 0x540) = TX0;
    REG(I2S + 0x550) = 2;
    REG(I2S + 0x304) = (1u << 1) | (1u << 2) | (1u << 5);
    REG(I2S + 0x500) = 1;
    REG(I2S + 0x000) = 1;
    while (stopped == 0)
        ;

    CHECK(updates == 3 && stopped == 1);
    CHECK(REG(RX0) == REG(TX0) && REG(RX0 + 4) == REG(TX0 + 4));
    CHECK(REG(RX1) == REG(TX1) && REG(RX1 + 4) == REG(TX1 + 4));
    CHECK(REG(RX0 + 8) == 0xaa5555aa && REG(RX1 + 8) == 0x55aaaa55);
    report("I2S OK\n", 7);
    __asm volatile("bkpt 0");
}

__attribute__((section(".vectors"), used))
const uintptr_t vectors[54] = {
    [0] = 0x20040000u,
    [1] = (uintptr_t)reset,
    [16 + 37] = (uintptr_t)i2s_irq,
};
