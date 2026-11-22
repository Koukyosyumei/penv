.syntax unified
.thumb

.section .vectors,"a",%progbits
.word _stack_top
.word reset_handler
.word firmware_fault  /* NMI */
.word firmware_fault  /* HardFault */
.word firmware_fault  /* MemManage */
.word firmware_fault  /* BusFault */
.word firmware_fault  /* UsageFault */
.word 0, 0, 0, 0
.word vPortSVCHandler
.word firmware_fault  /* DebugMon */
.word 0
.word xPortPendSVHandler
.word xPortSysTickHandler
.rept 19
.word firmware_fault
.endr
.word can1_tx_irq
.word can1_rx0_irq
.word firmware_fault  /* CAN1 RX1 */
.word can1_sce_irq
.rept 5
.word firmware_fault
.endr
.word tim2_irq
.rept 23
.word firmware_fault
.endr
.word uart4_irq
.rept 12
.word firmware_fault
.endr
.word can2_rx1_irq
.rept 16
.word firmware_fault
.endr

.section .text.reset_handler,"ax",%progbits
.global reset_handler
.thumb_func
reset_handler:
    ldr r0, =_data_load
    ldr r1, =_data_start
    ldr r2, =_data_end
1:
    cmp r1, r2
    bhs 2f
    ldr r3, [r0], #4
    str r3, [r1], #4
    b 1b
2:
    ldr r1, =_bss_start
    ldr r2, =_bss_end
    movs r3, #0
3:
    cmp r1, r2
    bhs 4f
    str r3, [r1], #4
    b 3b
4:
    ldr r0, =0xe000ed08
    ldr r1, =0x08000000
    str r1, [r0]
    dsb
    isb
    bl firmware_main
    b firmware_fault
