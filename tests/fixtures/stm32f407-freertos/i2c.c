#include "FreeRTOS.h"

#define REG(address) (*(volatile uint32_t *)(address))
#define I2C(offset) (*(volatile uint16_t *)(0x40005400U + (offset)))
#define I2C_CR1 I2C(0x00)
#define I2C_CR2 I2C(0x04)
#define I2C_DR I2C(0x10)
#define I2C_SR1 I2C(0x14)
#define I2C_SR2 I2C(0x18)
#define I2C_CCR I2C(0x1c)
#define I2C_TRISE I2C(0x20)

enum {
    PE = 1, START = 1U << 8, STOP = 1U << 9, ACK = 1U << 10, POS = 1U << 11,
    ITERREN = 1U << 8, ITEVTEN = 1U << 9,
    SB = 1, ADDR = 2, BTF = 4, RXNE = 1U << 6, TXE = 1U << 7, AF = 1U << 10,
    MSL = 1, BUSY = 2, TRA = 4,
    ECHO_ADDRESS = 0x10, ABSENT_ADDRESS = 0x11
};

enum transaction { WRITE_ECHO, READ_ECHO, READ_ABSENT };
static volatile enum transaction transaction;
static volatile unsigned event_count, error_count;
static unsigned tx_index;
static volatile uint8_t received[2];

/* Adapted from the Renode I2C echo test in tests/platforms/zynqmp_linux.robot. */
static const uint8_t echo_payload[] = {0x01, 0x23, 0x45};

void i2c1_event_irq(void)
{
    const uint16_t sr1 = I2C_SR1;
    if (sr1 & SB) {
        if (transaction == WRITE_ECHO) {
            I2C_DR = ECHO_ADDRESS << 1;
        } else {
            unsigned target = transaction == READ_ECHO ? ECHO_ADDRESS : ABSENT_ADDRESS;
            I2C_DR = (target << 1) | 1;
        }
        return;
    }

    if (sr1 & ADDR) {
        uint16_t direction = transaction == WRITE_ECHO ? TRA : 0;
        if (transaction == READ_ECHO) {
            I2C_CR1 = PE | POS; /* NACK the second byte of the two-byte read. */
        }
        configASSERT((I2C_SR2 & (MSL | BUSY | TRA)) == (MSL | BUSY | direction));
        if (transaction == WRITE_ECHO) {
            configASSERT((I2C_SR1 & TXE) != 0);
            I2C_DR = echo_payload[tx_index++];
        }
        return;
    }

    configASSERT((sr1 & BTF) != 0);
    if (transaction == WRITE_ECHO) {
        if (tx_index < sizeof echo_payload) {
            I2C_DR = echo_payload[tx_index++];
        } else {
            transaction = READ_ECHO;
            I2C_CR1 = PE | POS | ACK | START;
        }
    } else {
        configASSERT(transaction == READ_ECHO && (sr1 & RXNE) != 0);
        I2C_CR1 = PE | POS | STOP;
        received[0] = (uint8_t)I2C_DR;
        received[1] = (uint8_t)I2C_DR;
        ++event_count;
    }
}

void i2c1_error_irq(void)
{
    configASSERT(transaction == READ_ABSENT);
    configASSERT((I2C_SR1 & (AF | ADDR)) == AF);
    I2C_SR1 = (uint16_t)~AF;
    I2C_CR1 = PE | STOP;
    ++error_count;
}

void firmware_i2c_test(void)
{
    I2C_CR2 = 16;   /* PCLK1 at the fixture's divided 16 MHz APB1 clock. */
    I2C_CCR = 80;   /* Standard-mode divider for 100 kHz. */
    I2C_TRISE = 17;
    I2C_CR1 = PE;
    REG(0xe000e100) = 1U << 31; /* I2C1 event IRQ */
    REG(0xe000e104) = 1;       /* I2C1 error IRQ */
    I2C_CR2 = 16 | ITEVTEN | ITERREN;
    I2C_CR1 = PE | START;
    while (event_count != 1) {
    }
    configASSERT(received[0] == 0x01 && received[1] == 0x23);
    configASSERT((I2C_SR1 & (RXNE | BTF)) == 0 && (I2C_SR2 & BUSY) == 0);

    transaction = READ_ABSENT;
    I2C_CR1 = PE | START;
    while (error_count != 1) {
    }
    configASSERT((I2C_SR1 & AF) == 0 && (I2C_SR2 & BUSY) == 0);
    I2C_CR2 = 16;
}
