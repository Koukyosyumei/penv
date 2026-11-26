#include "FreeRTOS.h"

#define REG(address) (*(volatile uint32_t *)(address))
#define CAN1 0x40006400U
#define CAN2 0x40006800U
#define CAN(base, offset) REG((base) + (offset))
#define CAN_MCR 0x00
#define CAN_MSR 0x04
#define CAN_TSR 0x08
#define CAN_RF0R 0x0c
#define CAN_RF1R 0x10
#define CAN_IER 0x14
#define CAN_BTR 0x1c
#define CAN_FMR 0x200
#define CAN_FS1R 0x20c
#define CAN_FFA1R 0x214
#define CAN_FA1R 0x21c
#define CAN_FILTER(bank, word) (0x240 + 8 * (bank) + 4 * (word))

enum {
    INRQ = 1, SLEEP = 2, SLAKI = 1U << 4,
    RQCP0 = 1, TXOK0 = 2, RFOM = 1U << 5,
    TMEIE = 1, FMPIE0 = 1U << 1, FMPIE1 = 1U << 4, SLKIE = 1U << 17,
    ID_MASK = 0xffe00006U,
    CAN1_TX_IRQ = 19, CAN1_RX0_IRQ = 20, CAN1_SCE_IRQ = 22, CAN2_RX1_IRQ = 65
};

static volatile unsigned tx_count, rx0_count, rx1_count, sleep_count;

static void enable_irq(unsigned irq)
{
    REG(0xe000e100 + 4 * (irq / 32)) = 1U << (irq % 32);
}

void can1_tx_irq(void)
{
    configASSERT((CAN(CAN1, CAN_TSR) & (RQCP0 | TXOK0)) == (RQCP0 | TXOK0));
    CAN(CAN1, CAN_TSR) = RQCP0;
    ++tx_count;
}

void can1_rx0_irq(void)
{
    configASSERT((CAN(CAN1, CAN_RF0R) & 3U) == 1);
    configASSERT(CAN(CAN1, 0x1b0) == (0x123U << 21));
    configASSERT((CAN(CAN1, 0x1b4) & 0xfU) == 4);
    configASSERT(CAN(CAN1, 0x1b8) == 0x44332211);
    CAN(CAN1, CAN_RF0R) = RFOM;
    ++rx0_count;
}

void can2_rx1_irq(void)
{
    configASSERT((CAN(CAN2, CAN_RF1R) & 3U) == 1);
    configASSERT(CAN(CAN2, 0x1c0) == (0x456U << 21));
    configASSERT((CAN(CAN2, 0x1c4) & 0xfU) == 4);
    configASSERT(CAN(CAN2, 0x1c8) == 0x88776655);
    CAN(CAN2, CAN_RF1R) = RFOM;
    ++rx1_count;
}

void can1_sce_irq(void)
{
    configASSERT((CAN(CAN1, CAN_MSR) & SLAKI) != 0);
    CAN(CAN1, CAN_MSR) = SLAKI;
    ++sleep_count;
}

static void send(uint32_t base, uint32_t id, uint32_t payload)
{
    CAN(base, 0x184) = 4;
    CAN(base, 0x188) = payload;
    CAN(base, 0x180) = (id << 21) | 1;
}

void firmware_can_test(void)
{
    CAN(CAN1, CAN_MCR) = INRQ;
    CAN(CAN2, CAN_MCR) = INRQ;
    while ((CAN(CAN1, CAN_MSR) & INRQ) == 0 || (CAN(CAN2, CAN_MSR) & INRQ) == 0) {
    }
    // Internal loopback and silent mode, 16 tq per bit at 1 Mbit/s.
    CAN(CAN1, CAN_BTR) = 0xc02b0000U;
    CAN(CAN2, CAN_BTR) = 0xc02b0000U;

    CAN(CAN1, CAN_FMR) = (14U << 8) | 1;
    CAN(CAN1, CAN_FS1R) = (1U << 14) | 1;
    CAN(CAN1, CAN_FFA1R) = 1U << 14;
    CAN(CAN1, CAN_FILTER(0, 0)) = 0x123U << 21;
    CAN(CAN1, CAN_FILTER(0, 1)) = ID_MASK;
    CAN(CAN1, CAN_FILTER(14, 0)) = 0x456U << 21;
    CAN(CAN1, CAN_FILTER(14, 1)) = ID_MASK;
    CAN(CAN1, CAN_FA1R) = (1U << 14) | 1;
    CAN(CAN1, CAN_FMR) &= ~1U;

    CAN(CAN1, CAN_MCR) = 0;
    CAN(CAN2, CAN_MCR) = 0;
    while ((CAN(CAN1, CAN_MSR) & INRQ) != 0 || (CAN(CAN2, CAN_MSR) & INRQ) != 0) {
    }
    enable_irq(CAN1_TX_IRQ);
    enable_irq(CAN1_RX0_IRQ);
    enable_irq(CAN1_SCE_IRQ);
    enable_irq(CAN2_RX1_IRQ);
    CAN(CAN1, CAN_IER) = TMEIE | FMPIE0 | SLKIE;
    CAN(CAN2, CAN_IER) = FMPIE1;

    send(CAN1, 0x456, 0);  /* Matches CAN2's bank only. */
    while (tx_count != 1) {
    }
    configASSERT((CAN(CAN1, CAN_RF0R) & 3U) == 0 && rx0_count == 0);

    send(CAN1, 0x123, 0x44332211);
    send(CAN2, 0x456, 0x88776655);
    while (tx_count != 2 || rx0_count != 1 || rx1_count != 1) {
    }
    configASSERT((CAN(CAN2, CAN_TSR) & (RQCP0 | TXOK0)) == (RQCP0 | TXOK0));
    CAN(CAN1, CAN_MCR) = SLEEP;
    while (sleep_count != 1) {
    }
    CAN(CAN1, CAN_IER) = 0;
    CAN(CAN2, CAN_IER) = 0;
}
