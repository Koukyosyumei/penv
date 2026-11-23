.syntax unified
.thumb
.equ SHCSR, 0xe000ed24
.equ CFSR, 0xe000ed28
.equ HFSR, 0xe000ed2c
.equ BFAR, 0xe000ed38
.equ VTOR, 0xe000ed08
.equ ISER, 0xe000e100
.equ ISPR, 0xe000e200
.equ IABR, 0xe000e300
.equ UART, 0x300000
.equ MOCK, 0x100000

.section .vectors,"a",%progbits
.word 0x1000, reset, 0, hard_fault, 0, bus_fault

.section .boot,"ax",%progbits
.global reset
.thumb_func
reset:
#if CASE == 2
    ldr r0, =MOCK + 12
    ldr r1, =hard_fault
    str r1, [r0]
    ldr r0, =VTOR
    mov.w r1, #MOCK
    str r1, [r0]
    ldr r0, =ISER
    movs r1, #1
    str r1, [r0]
    b pend_irq
#else
    ldr r0, =SHCSR
    mov.w r1, #0x20000
    str r1, [r0]
    ldr r0, =BFAR
    ldr r1, =0x12345678
    str r1, [r0]
#if CASE == 1
    /* MOV.W r2,#1 would alter R2 if its second halfword were read. */
    ldr r0, =MOCK
    movw r1, #0xf04f
    strh r1, [r0]
#endif
    movs r2, #0x11
    ldr r0, =MOCK + 1
    bx r0
#endif
.ltorg

.section .fault,"ax",%progbits
pend_irq:
    ldr r0, =ISPR
    str r1, [r0]
interrupted_pc:
    b interrupted_pc
.ltorg

.section .busfault,"ax",%progbits
.thumb_func
bus_fault:
#if CASE == 2
    b fail
#else
    b check_instruction_fault
#endif
.ltorg

.section .hardfault,"ax",%progbits
.thumb_func
hard_fault:
#if CASE == 2
    mrs r0, ipsr
    cmp r0, #3
    bne fail
    mov r0, sp
    movw r1, #0xfe0
    cmp r0, r1
    bne fail
    ldr r0, [sp, #24]
    ldr r1, =interrupted_pc
    cmp r0, r1
    bne fail
    ldr r0, =CFSR
    ldr r1, [r0]
    cmp r1, #0
    bne fail
    ldr r0, =HFSR
    ldr r1, [r0]
    cmp r1, #2
    bne fail
    ldr r0, =ISPR
    ldr r1, [r0]
    tst r1, #1
    beq fail
    ldr r0, =IABR
    ldr r1, [r0]
    tst r1, #1
    bne fail
    b passed
#else
    b fail
#endif
.ltorg

.section .text,"ax",%progbits
#if CASE != 2
check_instruction_fault:
    mrs r0, ipsr
    cmp r0, #5
    bne fail
    ldr r0, =CFSR
    ldr r1, [r0]
    cmp.w r1, #0x100
    bne fail
    ldr r0, =BFAR
    ldr r1, [r0]
    ldr r0, =0x12345678
    cmp r1, r0
    bne fail
    ldr r0, =HFSR
    ldr r1, [r0]
    cmp r1, #0
    bne fail
    ldr r0, [sp, #8]
    cmp r0, #0x11
    bne fail
    ldr r0, [sp, #24]
    cmp.w r0, #MOCK
    bne fail
    b passed
#endif
passed:
    ldr r0, =success
    b print
fail:
    ldr r0, =failure
print:
    ldr r3, =UART
1:  ldrb r1, [r0], #1
    cbz r1, 2f
    strb r1, [r3]
    b 1b
2:  bkpt #0
.ltorg
success: .asciz "Fetch fault OK\n"
failure: .asciz "Fetch fault FAIL\n"
