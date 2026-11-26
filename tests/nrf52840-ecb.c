#include <stdint.h>

#define REG(address) (*(volatile uint32_t *)(address))
#define ECB 0x4000e000u
#define UART0 0x40002000u
#define BUFFER 0x20000020u

static volatile uint32_t ends;
static volatile uint32_t errors;

/* NIST SP 800-38A, F.1.1 (AES-128 ECB), blocks 1 and 2. */
static const uint8_t key[16] = {
    0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6,
    0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c,
};
static const uint8_t plaintext[2][16] = {
    {0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93, 0x17, 0x2a},
    {0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c, 0x9e, 0xb7, 0x6f, 0xac, 0x45, 0xaf, 0x8e, 0x51},
};
static const uint8_t ciphertext[2][16] = {
    {0x3a, 0xd7, 0x7b, 0xb4, 0x0d, 0x7a, 0x36, 0x60, 0xa8, 0x9e, 0xca, 0xf3, 0x24, 0x66, 0xef, 0x97},
    {0xf5, 0xd3, 0xd5, 0x85, 0x03, 0xb9, 0x69, 0x9d, 0xe7, 0x85, 0x89, 0x5a, 0x96, 0xfd, 0xba, 0xaf},
};

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
        report("ECB FAIL\n", 9); \
        __asm volatile("bkpt 0"); \
    } \
} while (0)

static void ecb_irq(void)
{
    if (REG(ECB + 0x100)) {
        REG(ECB + 0x100) = 0;
        ++ends;
    }
    if (REG(ECB + 0x104)) {
        REG(ECB + 0x104) = 0;
        ++errors;
    }
}

void reset(void)
{
    volatile uint8_t *buffer = (volatile uint8_t *)BUFFER;
    ends = errors = 0;
    buffer[48] = 0xa5;
    REG(0xe000e100u) = 1u << 14;
    REG(ECB + 0x504) = BUFFER;
    REG(ECB + 0x304) = 3;
    REG(ECB) = 0;
    CHECK(ends == 0 && errors == 0);

    for (unsigned block = 0; block < 2; ++block) {
        for (unsigned index = 0; index < 16; ++index) {
            buffer[index] = key[index];
            buffer[16 + index] = plaintext[block][index];
        }
        if (block == 1)
            REG(ECB + 0x308) = 1;
        REG(ECB) = 1;
        if (block == 1) {
            CHECK(ends == 1 && REG(ECB + 0x100) == 1);
            REG(ECB + 0x304) = 1;
        }
        CHECK(ends == block + 1 && errors == 0);
        for (unsigned index = 0; index < 16; ++index) {
            CHECK(buffer[index] == key[index]);
            CHECK(buffer[16 + index] == plaintext[block][index]);
            CHECK(buffer[32 + index] == ciphertext[block][index]);
        }
        CHECK(buffer[48] == 0xa5);
    }

    REG(0x2003fffcu) = 0x12345678;
    REG(ECB + 0x504) = 0x2003ffe0;
    REG(ECB) = 1;
    CHECK(ends == 2 && errors == 1 && REG(0x2003fffcu) == 0x12345678);
    report("ECB OK\n", 7);
    __asm volatile("bkpt 0");
}

__attribute__((section(".vectors"), used))
const uintptr_t vectors[31] = {
    [0] = 0x20040000u,
    [1] = (uintptr_t)reset,
    [16 + 14] = (uintptr_t)ecb_irq,
};
