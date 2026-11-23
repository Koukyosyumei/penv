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
.rept 14
.word firmware_fault
.endr
.word dma1_stream3_irq
.rept 4
.word firmware_fault
.endr
.word can1_tx_irq
.word can1_rx0_irq
.word firmware_fault  /* CAN1 RX1 */
.word can1_sce_irq
.word firmware_fault  /* EXTI9_5 */
.word tim9_irq
.rept 3
.word firmware_fault
.endr
.word tim2_irq
.rept 2
.word firmware_fault
.endr
.word i2c1_event_irq
.word i2c1_error_irq
.rept 2
.word firmware_fault
.endr
.word spi1_irq
.rept 16
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

.section .noinit,"aw",%nobits
.balign 4
.global watchdog_marker
.type watchdog_marker, %object
watchdog_marker:
    .space 4
.size watchdog_marker, . - watchdog_marker

#ifdef F407_FPU
.fpu fpv4-sp-d16
.section .text.fp_delay,"ax",%progbits
.global fp_delay
.thumb_func
fp_delay:
    push {r4-r6, lr}
    vpush {s30-s31}
    vmrs r6, fpscr
    mov r4, r1
    eor r5, r1, #0x80000000
    vmov s0, r4
    vmov s31, r5
    vmsr fpscr, r4
    bl vTaskDelay
    vmov r0, s0
    cmp r0, r4
    bne firmware_fault
    vmov r0, s31
    cmp r0, r5
    bne firmware_fault
    vmrs r0, fpscr
    cmp r0, r4
    bne firmware_fault
    vmsr fpscr, r6
    vpop {s30-s31}
    pop {r4-r6, pc}
#endif
