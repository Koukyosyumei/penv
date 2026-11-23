#include <stdbool.h>

#include "FreeRTOS.h"
#include "queue.h"
#include "task.h"

#define REG(address) (*(volatile uint32_t *)(address))
#define FLASH_ACR REG(0x40023c00)
#define FLASH_CR  REG(0x40023c10)
#define FLASH_RDP (*(volatile uint8_t *)0x1fffc001)
#define FLASH_NWRP REG(0x1fffc008)
#define IWDG_KR   (*(volatile uint16_t *)0x40003000)
#define IWDG_PR   (*(volatile uint16_t *)0x40003004)
#define IWDG_RLR  (*(volatile uint16_t *)0x40003008)
#define IWDG_SR   (*(volatile uint16_t *)0x4000300c)
#define RCC(offset) REG(0x40023800U + (offset))
#define RCC_CR RCC(0x00)
#define RCC_PLLCFGR RCC(0x04)
#define RCC_CFGR RCC(0x08)
#define RCC_APB2RSTR RCC(0x24)
#define RCC_AHB1ENR RCC(0x30)
#define RCC_APB1ENR RCC(0x40)
#define RCC_APB2ENR RCC(0x44)
#define RCC_BDCR RCC(0x70)
#define RCC_CSR RCC(0x74)
#define PWR_CR    REG(0x40007000)
#define PWR_CSR   REG(0x40007004)
#define UART4_SR  REG(0x40004c00)
#define UART4_DR  REG(0x40004c04)
#define UART4_BRR REG(0x40004c08)
#define UART4_CR1 REG(0x40004c0c)
#define TIM9_DIER REG(0x4001400c)
#define TIM9_SR   REG(0x40014010)
#define TIM9_EGR  REG(0x40014014)
#define TIM2_CR1  REG(0x40000000)
#define TIM2_DIER REG(0x4000000c)
#define TIM2_SR   REG(0x40000010)
#define TIM2_EGR  REG(0x40000014)
#define TIM2_PSC  REG(0x40000028)
#define TIM2_ARR  REG(0x4000002c)
#define NVIC_ISER(irq) REG(0xe000e100 + 4 * ((irq) / 32))
#define NVIC_IPR(irq) (*(volatile uint8_t *)(0xe000e400 + (irq)))

enum { TIM2_IRQ = 28, UART4_IRQ = 52 };
enum { UART_RXNE = 1U << 5, UART_RXNEIE = 1U << 5, UART_RE = 1U << 2 };
enum { TIM_UPDATE = 1, TIM_CEN = 1 };

enum source { TIMER_EVENT, UART_EVENT };
struct event {
    enum source source;
    uint32_t value;
};

static QueueHandle_t events;

enum { WATCHDOG_ARMED = 0x49adca11, WATCHDOG_FED = 0x49adfead };
extern volatile uint32_t watchdog_marker;

void firmware_can_test(void);
void firmware_spi_test(void);
void firmware_i2c_test(void);

static void enable_irq(unsigned irq)
{
    NVIC_IPR(irq) = configMAX_SYSCALL_INTERRUPT_PRIORITY;
    NVIC_ISER(irq) = 1U << (irq % 32);
}

static uint32_t uart_init(void)
{
    RCC_AHB1ENR = (1U << 21) | (1U << 2);  /* RCC AHB1ENR: DMA1, GPIOC */
    /* RCC APB1ENR: PWR, CAN1/2, I2C1, UART4, SPI2/3, TIM2 */
    RCC_APB1ENR = (1U << 28) | (3U << 25) | (1U << 21) | (1U << 19) | (3U << 14) | 1U;
    RCC_APB2ENR = (1U << 16) | (1U << 12);  /* TIM9, SPI1 */
    (void)RCC_APB2ENR;
    REG(0x40020824) = 0x8800;    /* GPIOC AFRH: PC10/PC11 AF8 */
    REG(0x40020800) = 0xa00000;  /* GPIOC MODER: PC10/PC11 alternate function */
    const uint32_t previous = UART4_CR1;
    UART4_BRR = 139;             /* PCLK1 16 MHz, 115200 baud */
    UART4_CR1 = (1U << 13) | (1U << 3);  /* UE | TE */
    return previous;
}

static void uart_char(char ch)
{
    while ((UART4_SR & (1U << 7)) == 0) {
    }
    UART4_DR = (uint8_t)ch;
}

static void uart_text(const char *text)
{
    while (*text != '\0') {
        uart_char(*text++);
    }
}

void firmware_fault(void)
{
    uart_text("FAULT\n");
    for (;;) {
    }
}

static void send_from_irq(enum source source, uint32_t value)
{
    const struct event event = {source, value};
    BaseType_t task_woken = pdFALSE;
    configASSERT(xQueueSendFromISR(events, &event, &task_woken) == pdPASS);
    portYIELD_FROM_ISR(task_woken);
}

void tim2_irq(void)
{
    static unsigned count;
    configASSERT((TIM2_SR & TIM_UPDATE) != 0);
    TIM2_SR = (uint32_t)~TIM_UPDATE;
    if (++count == 3) {
        TIM2_CR1 = 0;
    }
    send_from_irq(TIMER_EVENT, count);
}

void uart4_irq(void)
{
    configASSERT((UART4_SR & UART_RXNE) != 0);
    send_from_irq(UART_EVENT, UART4_DR & 0xff);
}

static void trigger_watchdog_reset(void)
{
    watchdog_marker = WATCHDOG_ARMED;
    uart_text("IWDG armed\n");
    IWDG_KR = 0xcccc;
    IWDG_KR = 0x5555;
    IWDG_PR = 0;
    IWDG_RLR = 47; /* 48 counts at 32 kHz / 4 gives 6 ms. */
    while (IWDG_SR != 0) {
    }
    IWDG_KR = 0xaaaa;
    vTaskDelay(4);
    IWDG_KR = 0xaaaa;
    vTaskDelay(3);
    /* Survived the original deadline. Wait without another refresh. */
    watchdog_marker = WATCHDOG_FED;
    uart_text("IWDG refresh held\n");
    for (;;) {
        vTaskDelay(portMAX_DELAY);
    }
}

static void event_task(void *arg)
{
    (void)arg;
    unsigned timer_count = 0;

    // Transfer the prescaler, clear UIF from UG, then enable update IRQ.
    TIM2_PSC = 31999;
    TIM2_ARR = 3;
    TIM2_EGR = TIM_UPDATE;
    TIM2_SR = 0;
    TIM2_DIER = TIM_UPDATE;
    enable_irq(TIM2_IRQ);
    TIM2_CR1 = TIM_CEN;
    enable_irq(UART4_IRQ);
    UART4_CR1 |= UART_RE | UART_RXNEIE;

    for (;;) {
        struct event event;
        configASSERT(xQueueReceive(events, &event, portMAX_DELAY) == pdPASS);
        if (event.source == TIMER_EVENT) {
            configASSERT(event.value == ++timer_count);
            uart_text("TIM2 queue=");
            uart_char((char)('0' + timer_count));
        } else {
            configASSERT(event.source == UART_EVENT);
            if (event.value == '!') {
                trigger_watchdog_reset();
            }
            uart_text("UART4 echo=");
            uart_char((char)event.value);
        }
        uart_char('\n');
    }
}

static void power_test(void)
{
    enum { DBP = 1U << 8, VOS = 1U << 14, VOSRDY = 1U << 14,
           BRE = 1U << 9, BRR = 1U << 3 };

    configASSERT((PWR_CR & (VOS | DBP)) == VOS);
    PWR_CSR = BRE;
    configASSERT((PWR_CSR & (BRE | BRR)) == 0);
    PWR_CR = VOS | DBP;
    configASSERT((PWR_CR & DBP) != 0);
    PWR_CSR = BRE;
    while ((PWR_CSR & (VOSRDY | BRR)) != (VOSRDY | BRR)) {
    }
    PWR_CSR = 0;
    PWR_CR = VOS;
    configASSERT((PWR_CSR & BRE) == 0 && (PWR_CR & DBP) == 0);
}

static volatile unsigned tim9_interrupts;

void tim9_irq(void)
{
    configASSERT((TIM9_SR & TIM_UPDATE) != 0);
    TIM9_SR = (uint32_t)~TIM_UPDATE;
    ++tim9_interrupts;
    __asm volatile ("cpsid f" ::: "memory");
}

/* Adapt the two STM32F4_Discovery.robot FAULTMASK cases with TIM9 clocked. */
static void faultmask_test(void)
{
    TIM9_DIER = TIM_UPDATE;
    NVIC_ISER(24) = 1U << 24;
    __asm volatile ("cpsid if" ::: "memory");
    __asm volatile ("cpsie i" ::: "memory");
    TIM9_EGR = TIM_UPDATE;
    configASSERT((REG(0xe000e200) & (1U << 24)) != 0);
    configASSERT(tim9_interrupts == 0);
    __asm volatile ("cpsie if" ::: "memory");
    while (tim9_interrupts != 1) {
    }

    TIM9_EGR = TIM_UPDATE;
    while (tim9_interrupts != 2) {
    }
    TIM9_DIER = 0;
}

#ifdef F407_UPSTREAM_BOARD
static void board_wiring_test(void)
{
    static volatile uint32_t data;
    const uintptr_t byte_offset = (uintptr_t)&data - 0x20000000U;
    data = 0xa5;
    REG(0x22000000U + byte_offset * 32U + 8U * 4U) = 1;
    configASSERT(data == 0x1a5);
    configASSERT(REG(0x22000000U + byte_offset * 32U + 8U * 4U) == 1);
    REG(0x22000000U + byte_offset * 32U + 8U * 4U) = 0;
    configASSERT(data == 0xa5);

    RCC_AHB1ENR |= 1U << 3; /* GPIOD and the on-board LED on PD12. */
    REG(0x40020c00) |= 1U << 24;
    const uintptr_t led_alias = 0x42000000U + (0x20c14U * 32U) + (12U * 4U);
    REG(led_alias) = 1;
    configASSERT((REG(0x40020c10) & (1U << 12)) != 0);
    REG(led_alias) = 0;
    configASSERT((REG(0x40020c10) & (1U << 12)) == 0);
}
#endif

static void clock_test(bool restarted)
{
    enum { HSEON = 1U << 16, HSERDY = 1U << 17, PLLON = 1U << 24, PLLRDY = 1U << 25,
           IWDGRSTF = 1U << 29, RMVF = 1U << 24, DBP = 1U << 8, VOS = 1U << 14,
           LSE_ON = 1, LSE_RDY = 2, RTC_LSE = 1U << 8, RTC_EN = 1U << 15, BDRST = 1U << 16 };
    configASSERT(((RCC_CSR & IWDGRSTF) != 0) == restarted);
    RCC_CSR = RMVF;
    configASSERT((RCC_CSR & 0xfe000000U) == 0);
    configASSERT((RCC_AHB1ENR & (1U << 21)) != 0 && (RCC_APB1ENR & 1U) != 0);

    RCC_APB2ENR &= ~(1U << 12);
    configASSERT(REG(0x40013010) == 0); /* Clock-gated SPI1 CRCPR */
    RCC_APB2ENR |= 1U << 12;
    configASSERT(REG(0x40013010) == 7);

    REG(0x4001402c) = 123;
    RCC_APB2RSTR = 1U << 16;
    configASSERT(REG(0x4001402c) == 0);
    RCC_APB2RSTR = 0;
    configASSERT(REG(0x4001402c) == 0xffff);

    RCC_BDCR = LSE_ON | RTC_LSE | RTC_EN;
    configASSERT(RCC_BDCR == 0); /* PWR DBP is still clear. */
    PWR_CR = VOS | DBP;
    (void)PWR_CR;
    RCC_BDCR = LSE_ON | RTC_LSE | RTC_EN;
    while ((RCC_BDCR & LSE_RDY) == 0) {
    }
    REG(0x40002850) = 0xa1b2c3d4;
    configASSERT(REG(0x40002850) == 0xa1b2c3d4);
    RCC_BDCR = BDRST;
    RCC_BDCR = 0;
    RCC_BDCR = LSE_ON | RTC_LSE | RTC_EN;
    configASSERT(REG(0x40002850) == 0);
    RCC_BDCR = BDRST;
    RCC_BDCR = 0;
    PWR_CR = VOS;

    // HSE 8 MHz / 8 x 192 / 6 gives 32 MHz. APB1 and APB2 divide by two.
    RCC_CR |= HSEON;
    while ((RCC_CR & HSERDY) == 0) {
    }
    RCC_PLLCFGR = 8U | (192U << 6) | (2U << 16) | (1U << 22) | (4U << 24);
    RCC_CR |= PLLON;
    while ((RCC_CR & PLLRDY) == 0) {
    }
    RCC_CFGR = (4U << 10) | (4U << 13) | 2U;
    while ((RCC_CFGR & 0x0c) != 8U) {
    }
}

struct worker {
    char name;
    TickType_t first;
    TickType_t period;
};

static void task(void *arg)
{
    const struct worker *worker = arg;
    vTaskDelay(worker->first);

    for (unsigned iteration = 0; iteration < 3; ++iteration) {
        const TickType_t expected = worker->first + iteration * worker->period;
        taskENTER_CRITICAL();
        const TickType_t tick = xTaskGetTickCount();
        configASSERT(tick == expected);
        uart_text("task ");
        uart_char(worker->name);
        uart_text(" tick=");
        uart_char((char)('0' + tick / 10));
        uart_char((char)('0' + tick % 10));
        uart_char('\n');
        taskEXIT_CRITICAL();
        vTaskDelay(worker->period);
    }

    for (;;) {
        vTaskDelay(portMAX_DELAY);
    }
}

void vApplicationGetIdleTaskMemory(StaticTask_t **tcb, StackType_t **stack,
                                   configSTACK_DEPTH_TYPE *depth)
{
    static StaticTask_t idle_tcb;
    static StackType_t idle_stack[configMINIMAL_STACK_SIZE];
    *tcb = &idle_tcb;
    *stack = idle_stack;
    *depth = configMINIMAL_STACK_SIZE;
}

void firmware_main(void)
{
    static struct worker workers[] = { {'A', 1, 4}, {'B', 3, 4} };
    static StackType_t stacks[2][configMINIMAL_STACK_SIZE];
    static StaticTask_t tasks[2];

    static StaticQueue_t queue;
    static uint8_t queue_storage[4 * sizeof(struct event)];
    static StackType_t event_stack[configMINIMAL_STACK_SIZE];
    static StaticTask_t event_tcb;

    /* Check reset state before the firmware writes these peripheral registers. */
    const uint32_t systick = REG(0xe000e010);
    const uint32_t irq_enables = REG(0xe000e100) | REG(0xe000e104) | REG(0xe000e108);
    const uint32_t clock_state = RCC_CFGR | (RCC_CR & 0x03030000U);
    const uint32_t clock_gates = RCC_APB1ENR | RCC_APB2ENR;
    const uint32_t uart_control = uart_init();
    configASSERT((systick & 3) == 0 && irq_enables == 0 && uart_control == 0);
    configASSERT(IWDG_PR == 4 && IWDG_RLR == 0xfff && IWDG_SR == 0);
    configASSERT(clock_state == 0 && clock_gates == 0);
    const bool restarted = watchdog_marker != 0;
    if (restarted) {
        configASSERT(watchdog_marker == WATCHDOG_FED);
        watchdog_marker = 0;
        uart_text("IWDG reboot: SRAM retained\n");
    }
    FLASH_ACR = 1;
    configASSERT((FLASH_ACR & 0xf) == 1);
    configASSERT((FLASH_CR & (1U << 31)) != 0);
    configASSERT(FLASH_RDP == 0xaa && (FLASH_NWRP & 0xfff) == 0xfff);
    uart_text("STM32F407 FreeRTOS " tskKERNEL_VERSION_NUMBER "\n");
    clock_test(restarted);
#ifdef F407_UPSTREAM_BOARD
    board_wiring_test();
    uart_text("Discovery bit-band GPIO and SRAM OK\n");
#endif
    uart_text("RCC PLL 32 MHz, RTC and TIM9 OK\n");
    faultmask_test();
    uart_text("TIM9 FAULTMASK OK\n");
    power_test();
    uart_text("PWR ready OK\n");
    firmware_can_test();
    uart_text("CAN loopback OK\n");
    firmware_spi_test();
    uart_text("SPI IRQ and DMA OK\n");
    firmware_i2c_test();
#ifdef F407_UPSTREAM_BOARD
    uart_text("I2C unpopulated bus NACK OK\n");
#else
    uart_text("I2C echo 01 23 and NACK OK\n");
#endif
    configASSERT(xTaskCreateStatic(task, "A", configMINIMAL_STACK_SIZE, &workers[0],
                                   2, stacks[0], &tasks[0]) != NULL);
    configASSERT(xTaskCreateStatic(task, "B", configMINIMAL_STACK_SIZE, &workers[1],
                                   2, stacks[1], &tasks[1]) != NULL);
    events = xQueueCreateStatic(4, sizeof(struct event), queue_storage, &queue);
    configASSERT(events != NULL);
    configASSERT(xTaskCreateStatic(event_task, "events", configMINIMAL_STACK_SIZE,
                                   NULL, 3, event_stack, &event_tcb) != NULL);
    vTaskStartScheduler();
    firmware_fault();
}
