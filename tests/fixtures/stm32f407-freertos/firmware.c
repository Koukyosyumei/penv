#include "FreeRTOS.h"
#include "task.h"

#define REG(address) (*(volatile uint32_t *)(address))
#define UART4_SR  REG(0x40004c00)
#define UART4_DR  REG(0x40004c04)
#define UART4_BRR REG(0x40004c08)
#define UART4_CR1 REG(0x40004c0c)

static void uart_init(void)
{
    REG(0x40023830) = 1U << 2;   /* RCC AHB1ENR: GPIOC */
    REG(0x40023840) = 1U << 19;  /* RCC APB1ENR: UART4 */
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

    uart_init();
    uart_text("STM32F407 FreeRTOS " tskKERNEL_VERSION_NUMBER "\n");
    configASSERT(xTaskCreateStatic(task, "A", configMINIMAL_STACK_SIZE, &workers[0],
                                   2, stacks[0], &tasks[0]) != NULL);
    configASSERT(xTaskCreateStatic(task, "B", configMINIMAL_STACK_SIZE, &workers[1],
                                   2, stacks[1], &tasks[1]) != NULL);
    vTaskStartScheduler();
    firmware_fault();
}
