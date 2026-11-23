#include <stdint.h>

#define REG(address) (*(volatile uint32_t *)(address))
#define SPI2 0x40023000u
#define UART0 0x40002000u

static volatile uint32_t received_count;
static volatile uint32_t received[2];

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
        report("SPI FAIL\n", 9); \
        __asm volatile("bkpt 0"); \
    } \
} while (0)

static void spi2_irq(void)
{
    REG(SPI2 + 0x108) = 0;
    uint32_t byte = REG(SPI2 + 0x518);
    if (received_count < 2)
        received[received_count++] = byte;
}

void reset(void)
{
    received_count = 0;
    REG(0xe000e104u) = 1u << (35 - 32);
    REG(SPI2 + 0x51c) = 0xff;
    CHECK(REG(SPI2 + 0x108) == 0);

    REG(SPI2 + 0x500) = 1;
    REG(SPI2 + 0x524) = 0x10000000;
    REG(SPI2 + 0x554) = 7;
    CHECK(REG(SPI2 + 0x524) == 0x10000000 && REG(SPI2 + 0x554) == 7);
    REG(SPI2 + 0x51c) = 0xa5;
    REG(SPI2 + 0x51c) = 0x5a;
    CHECK(REG(SPI2 + 0x108) == 1 && received_count == 0);

    REG(SPI2 + 0x304) = 1u << 2;
    CHECK(received_count == 2 && received[0] == 0xa5 && received[1] == 0x5a);
    CHECK(REG(SPI2 + 0x108) == 0 && REG(SPI2 + 0x518) == 0);
    REG(SPI2 + 0x308) = 1u << 2;
    CHECK(REG(SPI2 + 0x304) == 0);

    report("SPI OK\n", 7);
    __asm volatile("bkpt 0");
}

__attribute__((section(".vectors"), used))
const uintptr_t vectors[52] = {
    [0] = 0x20040000u,
    [1] = (uintptr_t)reset,
    [16 + 35] = (uintptr_t)spi2_irq,
};
