#ifndef FREERTOS_CONFIG_H
#define FREERTOS_CONFIG_H

#define configCPU_CLOCK_HZ                     32000000UL
#define configTICK_RATE_HZ                     1000
#define configTICK_TYPE_WIDTH_IN_BITS          TICK_TYPE_WIDTH_32_BITS
#define configUSE_PREEMPTION                   1
#define configMAX_PRIORITIES                   4
#define configMINIMAL_STACK_SIZE               128
#define configMAX_TASK_NAME_LEN                8
#define configSUPPORT_STATIC_ALLOCATION        1
#define configSUPPORT_DYNAMIC_ALLOCATION       0
#define configUSE_IDLE_HOOK                    0
#define configUSE_TICK_HOOK                    0
#define configUSE_TIMERS                       0
#define configMAX_SYSCALL_INTERRUPT_PRIORITY   0x50
#define INCLUDE_vTaskDelay                     1
#define INCLUDE_vTaskSuspend                   1

void firmware_fault(void);
#define configASSERT(condition) do { if (!(condition)) firmware_fault(); } while (0)

#endif
