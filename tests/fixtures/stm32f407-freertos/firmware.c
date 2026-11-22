#include "FreeRTOS.h"
#include "queue.h"
#include "task.h"

#define REG(address) (*(volatile uint32_t *)(address))
#define UART4_SR  REG(0x40004c00)
#define UART4_DR  REG(0x40004c04)
#define UART4_BRR REG(0x40004c08)
#define UART4_CR1 REG(0x40004c0c)
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

static void enable_irq(unsigned irq)
{
    NVIC_IPR(irq) = configMAX_SYSCALL_INTERRUPT_PRIORITY;
    NVIC_ISER(irq) = 1U << (irq % 32);
}

static void uart_init(void)
{
    REG(0x40023830) = 1U << 2;   /* RCC AHB1ENR: GPIOC */
    REG(0x40023840) = (1U << 19) | 1U;  /* RCC APB1ENR: UART4 and TIM2 */
    (void)REG(0x40023840);
    REG(0x40020824) = 0x8800;    /* GPIOC AFRH: PC10/PC11 AF8 */
    REG(0x40020800) = 0xa00000;  /* GPIOC MODER: PC10/PC11 alternate function */
    UART4_BRR = 139;             /* HSI 16 MHz, 115200 baud */
    UART4_CR1 = (1U << 13) | (1U << 3);  /* UE | TE */
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

static void event_task(void *arg)
{
    (void)arg;
    unsigned timer_count = 0;

    // Transfer the prescaler, clear UIF from UG, then enable update IRQ.
    TIM2_PSC = 15999;
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
            uart_text("UART4 echo=");
            uart_char((char)event.value);
        }
        uart_char('\n');
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

    uart_init();
    uart_text("STM32F407 FreeRTOS " tskKERNEL_VERSION_NUMBER "\n");
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
