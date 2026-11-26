#include <stdint.h>

#define UART1_REG(offset) (*(volatile uint32_t *)(0x40028000u + (offset)))
#define TASK_STARTRX 0x000
#define TASK_STARTTX 0x008
#define EVENT_RXDRDY 0x108
#define ENABLE 0x500
#define RXD 0x518
#define TXD 0x51c

static uint8_t read_byte(void)
{
    while (!UART1_REG(EVENT_RXDRDY)) {
    }
    UART1_REG(EVENT_RXDRDY) = 0;
    return UART1_REG(RXD);
}

static void send_command(uint16_t opcode, const uint8_t *parameters, uint8_t length)
{
    UART1_REG(TXD) = 1; /* H:4 command packet. */
    UART1_REG(TXD) = opcode & 0xff;
    UART1_REG(TXD) = opcode >> 8;
    UART1_REG(TXD) = length;
    for (unsigned i = 0; i < length; ++i) {
        UART1_REG(TXD) = parameters[i];
    }

    for (;;) {
        while (read_byte() != 4) { /* H:4 event packet. */
        }
        const uint8_t event = read_byte();
        const uint8_t event_length = read_byte();
        uint8_t complete[4] = {0};
        for (unsigned i = 0; i < event_length; ++i) {
            const uint8_t byte = read_byte();
            if (i < sizeof(complete)) {
                complete[i] = byte;
            }
        }
        if (event == 0x0e && event_length >= sizeof(complete)
                && ((uint16_t)complete[2] << 8 | complete[1]) == opcode) {
            if (complete[3] != 0) {
                __asm volatile("bkpt 0");
            }
            return;
        }
    }
}

void reset(void)
{
    static const uint8_t address[] = {0x01, 0x02, 0x03, 0x04, 0x05, 0xc6};
    static const uint8_t params[] = {
        0xa0, 0x00, 0xa0, 0x00, /* 100 ms interval. */
        2, 1, 0,                /* ADV_SCAN_IND, random own address, public peer. */
        0, 0, 0, 0, 0, 0,
        7, 0,                   /* All three channels, accept any scanner. */
    };
    static const uint8_t data[32] = {28, 27, 0xff, 0xff, 0xff};
    static const uint8_t enable[] = {1};

    UART1_REG(ENABLE) = 4;
    UART1_REG(TASK_STARTRX) = 1;
    UART1_REG(TASK_STARTTX) = 1;
    send_command(0x0c03, 0, 0);                      /* Reset. */
    send_command(0x2005, address, sizeof(address));  /* LE Set Random Address. */
    send_command(0x2006, params, sizeof(params));    /* LE Set Advertising Parameters. */
    send_command(0x2008, data, sizeof(data));        /* LE Set Advertising Data. */
    send_command(0x200a, enable, sizeof(enable));    /* LE Set Advertising Enable. */
    for (;;) {
        __asm volatile("wfi");
    }
}

__attribute__((section(".vectors"), used))
const uintptr_t vectors[] = {0x20040000u, (uintptr_t)reset};
