#include <stdint.h>

#define REG(address) (*(volatile uint32_t *)(address))
#define SAMPLE(address) (*(volatile uint16_t *)(address))
#define PDM 0x4001d000u
#define UART0 0x40002000u
#define BUFFER0 0x20000020u
#define BUFFER1 0x20000040u

static volatile uint32_t started;
static volatile uint32_t completed;
static volatile uint32_t stopped;

static void report(const char *text)
{
    volatile char *buffer = (volatile char *)0x20000000u;
    for (unsigned index = 0; index < 7; ++index)
        buffer[index] = text[index];
    REG(UART0 + 0x500) = 8;
    REG(UART0 + 0x544) = (uintptr_t)buffer;
    REG(UART0 + 0x548) = 7;
    REG(UART0 + 0x008) = 1;
}

#define CHECK(condition) do { \
    if (!(condition)) { \
        report("PDM BAD"); \
        __asm volatile("bkpt 0"); \
    } \
} while (0)

static void pdm_irq(void)
{
    if (REG(PDM + 0x100)) {
        REG(PDM + 0x100) = 0;
        if (++started == 1)
            REG(PDM + 0x560) = BUFFER1;
    }
    if (REG(PDM + 0x108)) {
        REG(PDM + 0x108) = 0;
        if (++completed == 2)
            REG(PDM + 0x004) = 1;
    }
    if (REG(PDM + 0x104)) {
        REG(PDM + 0x104) = 0;
        ++stopped;
    }
}

void reset(void)
{
    started = completed = stopped = 0;
    REG(BUFFER0 + 4) = 0xaaaa0000;
    REG(BUFFER1 + 4) = 0x5555ffff;
    REG(0xe000e100u) = 1u << 29;
    REG(PDM + 0x000) = 1;
    CHECK(REG(PDM + 0x100) == 0);
    REG(PDM + 0x508) = 1;
    REG(PDM + 0x560) = BUFFER0;
    REG(PDM + 0x564) = 3;
    REG(PDM + 0x304) = 7;
    REG(PDM + 0x500) = 1;
    REG(PDM + 0x000) = 1;
    while (!stopped)
        ;

    CHECK(started == 3 && completed == 2 && stopped == 1);
    CHECK(REG(BUFFER0) == 0x7fff9234 && SAMPLE(BUFFER0 + 4) == 0x8000);
    CHECK(REG(BUFFER1) == 0x0000abcd && SAMPLE(BUFFER1 + 4) == 0);
    CHECK(SAMPLE(BUFFER0 + 6) == 0xaaaa && SAMPLE(BUFFER1 + 6) == 0x5555);
    report("PDM OK\n");
    __asm volatile("bkpt 0");
}

__attribute__((section(".vectors"), used))
const uintptr_t vectors[46] = {
    [0] = 0x20040000u,
    [1] = (uintptr_t)reset,
    [16 + 29] = (uintptr_t)pdm_irq,
};
