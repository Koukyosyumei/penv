#include <stdint.h>

#define REG(address) (*(volatile uint32_t *)(address))
#define BYTE(address) (*(volatile uint8_t *)(address))
#define RADIO 0x40001000u
#define UART0 0x40002000u
#define TX 0x20000020u
#define RX 0x20000040u

static volatile uint32_t ends;
static volatile uint32_t good_crc;
static volatile uint32_t disabled;

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
        report("RADIO FAIL\n", 11); \
        __asm volatile("bkpt 0"); \
    } \
} while (0)

static void radio_irq(void)
{
    if (REG(RADIO + 0x10c)) {
        REG(RADIO + 0x10c) = 0;
        ++ends;
    }
    if (REG(RADIO + 0x130)) {
        REG(RADIO + 0x130) = 0;
        ++good_crc;
    }
    if (REG(RADIO + 0x110)) {
        REG(RADIO + 0x110) = 0;
        ++disabled;
    }
}

void reset(void)
{
    ends = good_crc = disabled = 0;
    CHECK(REG(RADIO + 0x66c) == 0x052d0000);
    REG(RADIO + 0x66c) = 0x123456ff;
    CHECK(REG(RADIO + 0x66c) == 0x052d0007);
    REG(RADIO + 0xffc) = 0;
    REG(RADIO + 0xffc) = 1;
    CHECK(REG(RADIO + 0x66c) == 0x052d0000);
    REG(TX) = 0x22110342;
    BYTE(TX + 4) = 0x33;
    BYTE(RX + 5) = 0xee;
    REG(0xe000e100u) = 1u << 1;
    REG(RADIO + 0x304) = (1u << 3) | (1u << 4) | (1u << 12);
    /* Reserved bits must read as zero without discarding valid fields.
     * Keep one S0 byte, an 8-bit LENGTH, and an 8-byte MAXLEN. */
    REG(RADIO + 0x514) = 0x80000108;
    REG(RADIO + 0x518) = 0x80000008;
    CHECK(REG(RADIO + 0x514) == 0x108);
    CHECK(REG(RADIO + 0x518) == 8);
    REG(RADIO + 0x504) = TX;
    REG(RADIO + 0x200) = 0x80000003; /* READY -> START; END -> DISABLE. */
    CHECK(REG(RADIO + 0x200) == 3);
    REG(RADIO + 0x000) = 1;
    CHECK(ends == 1 && disabled == 1 && good_crc == 0);
    CHECK(REG(RADIO + 0x100) == 1 && REG(RADIO + 0x154) == 1);
    CHECK(REG(RADIO + 0x104) == 1 && REG(RADIO + 0x108) == 1);
    REG(RADIO + 0x104) = 0;
    REG(RADIO + 0x108) = 0;

    REG(TX) = 0;
    BYTE(TX + 4) = 0;
    REG(RADIO + 0x504) = RX;
    REG(RADIO + 0x004) = 1;
    CHECK(ends == 2 && disabled == 2 && good_crc == 1);
    CHECK(REG(RADIO + 0x158) == 1 && REG(RADIO + 0x400) == 1);
    CHECK(REG(RADIO + 0x104) == 1 && REG(RADIO + 0x108) == 1);
    CHECK(REG(RADIO + 0x550) == 0);
    CHECK(REG(RX) == 0x22110342 && BYTE(RX + 4) == 0x33 && BYTE(RX + 5) == 0xee);

    REG(RADIO + 0x004) = 1;
    CHECK(ends == 2 && REG(RADIO + 0x550) == 3);
    REG(RADIO + 0x00c) = 1;
    CHECK(REG(RADIO + 0x550) == 2);
    report("RADIO OK\n", 9);
    __asm volatile("bkpt 0");
}

__attribute__((section(".vectors"), used))
const uintptr_t vectors[18] = {
    [0] = 0x20040000u,
    [1] = (uintptr_t)reset,
    [16 + 1] = (uintptr_t)radio_irq,
};
