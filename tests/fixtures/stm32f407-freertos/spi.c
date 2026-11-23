#include "FreeRTOS.h"

#define REG(address) (*(volatile uint32_t *)(address))
#define SPI1 0x40013000U
#define SPI2 0x40003800U
#define SPI3 0x40003c00U
#define SPI_CR1(base) REG((base) + 0x00)
#define SPI_CR2(base) REG((base) + 0x04)
#define SPI_SR(base)  REG((base) + 0x08)
#define SPI_DR8(base) (*(volatile uint8_t *)((base) + 0x0c))
#define SPI_DR16(base) (*(volatile uint16_t *)((base) + 0x0c))

#define DMA_LISR  REG(0x40026000)
#define DMA_LIFCR REG(0x40026008)
#define DMA_S3CR  REG(0x40026058)
#define DMA_S3NDTR REG(0x4002605c)
#define DMA_S3PAR REG(0x40026060)
#define DMA_S3M0AR REG(0x40026064)

enum {
    MASTER = 1U << 2, ENABLE = 1U << 6, SSM_SSI = 3U << 8, DFF = 1U << 11,
    RXNE = 1, TXE = 2, OVR = 1U << 6,
    RXDMAEN = 1, ERRIE = 1U << 5, RXNEIE = 1U << 6, TXEIE = 1U << 7,
    DMA_ENABLE = 1, DMA_TCIE = 1U << 4, DMA_MINC = 1U << 10,
    DMA_TCIF3 = 1U << 27, DMA_ALL_FLAGS3 = 0x3dU << 22
};

static volatile unsigned tx_count, rx_count, error_count, dma_count;
static volatile uint8_t dma_bytes[2];

void spi1_irq(void)
{
    const uint32_t enabled = SPI_CR2(SPI1);
    const uint32_t status = SPI_SR(SPI1);
    if ((enabled & ERRIE) && (status & OVR)) {
        configASSERT(SPI_DR8(SPI1) == 0x96);
        (void)SPI_SR(SPI1);
        configASSERT((SPI_SR(SPI1) & (RXNE | OVR)) == 0);
        ++error_count;
    } else if ((enabled & RXNEIE) && (status & RXNE)) {
        configASSERT(SPI_DR8(SPI1) == 0xa5);
        ++rx_count;
    } else {
        configASSERT((enabled & TXEIE) && (status & TXE));
        SPI_CR2(SPI1) = RXNEIE;
        SPI_DR8(SPI1) = 0xa5;
        ++tx_count;
    }
}

void dma1_stream3_irq(void)
{
    configASSERT((DMA_LISR & DMA_TCIF3) != 0);
    DMA_LIFCR = DMA_ALL_FLAGS3;
    ++dma_count;
}

void firmware_spi_test(void)
{
    const uint32_t master = MASTER | ENABLE | SSM_SSI;
    SPI_CR1(SPI1) = master;
    REG(0xe000e104) = 1U << (35 - 32); /* SPI1 IRQ */
    SPI_CR2(SPI1) = TXEIE;
    while (rx_count != 1) {
    }
    configASSERT(tx_count == 1);
    SPI_CR2(SPI1) = ERRIE;
    SPI_DR8(SPI1) = 0x96;
    SPI_DR8(SPI1) = 0x69;
    while (error_count != 1) {
    }
    SPI_CR2(SPI1) = 0;

    SPI_CR1(SPI3) = master | DFF;
    SPI_DR16(SPI3) = 0x12ab;
    configASSERT((SPI_SR(SPI3) & RXNE) != 0);
    configASSERT(SPI_DR16(SPI3) == 0x12ab);
    configASSERT((SPI_SR(SPI3) & RXNE) == 0);

    REG(0xe000e100) = 1U << 14; /* DMA1 stream 3 IRQ */
    DMA_S3PAR = SPI2 + 0x0c;
    DMA_S3M0AR = (uintptr_t)dma_bytes;
    DMA_S3NDTR = sizeof dma_bytes;
    DMA_S3CR = DMA_MINC | DMA_TCIE | DMA_ENABLE;
    SPI_CR1(SPI2) = master;
    SPI_CR2(SPI2) = RXDMAEN;
    SPI_DR8(SPI2) = 0x3c;
    SPI_DR8(SPI2) = 0xc3;
    while (dma_count != 1) {
    }
    configASSERT(dma_bytes[0] == 0x3c && dma_bytes[1] == 0xc3);
    configASSERT(DMA_S3NDTR == 0 && (SPI_SR(SPI2) & (RXNE | OVR)) == 0);
    SPI_CR2(SPI2) = 0;
}
