.syntax unified
.thumb
.fpu fpv4-sp-d16

/* CASE 0–10 select the upstream data-access, escalation and FP parameters. */
.equ SCB_SHCSR, 0xe000ed24
.equ SCB_CFSR, 0xe000ed28
.equ SCB_HFSR, 0xe000ed2c
.equ SCB_BFAR, 0xe000ed38
.equ SCB_CPACR, 0xe000ed88
.equ UART, 0x300000
#if CASE == 2
.equ fault_address, 0x200000
#else
.equ fault_address, 0x100000
#endif

.section .vectors,"a",%progbits
.word 0x1000, reset, 0, hard_fault, 0, bus_fault, 0

.section .boot,"ax",%progbits
.global reset
.thumb_func
reset:
#if CASE != 3
    ldr r3, =SCB_SHCSR
    mov.w r2, #0x20000
    str r2, [r3]
#endif
#if CASE >= 7
    ldr r3, =SCB_CPACR
    mov.w r2, #0xf00000
    str r2, [r3]
    dsb
    isb
#endif
    ldr r0, =fault_address
    movs r1, #0x11
    ldr r2, =0xa5a5a5a5
#if CASE == 5 || CASE == 6
    ldr r10, =fault_instruction
#endif
    b fault_instruction
.ltorg

.section .fault,"ax",%progbits
fault_instruction:
#if CASE == 1
    str r2, [r0]
#elif CASE == 5
    ldr r3, [r0]
#elif CASE == 6
    str r3, [r0]
#elif CASE == 7
    vstr s0, [r0]
#elif CASE == 8
    vstr d0, [r0]
#elif CASE == 9
    vldr s0, [r0]
#elif CASE == 10
    vldr d0, [r0]
#else
    ldr r2, [r0]
#endif
#if CASE == 5 || CASE == 6
    mov.w r10, #0
#else
    movs r1, #0x22
#endif
    b fail

.section .busfault,"ax",%progbits
.thumb_func
bus_fault:
#if CASE == 4
    ldr r2, [r0]
    movs r1, #0x22
    b fail
#elif CASE == 3
    b fail
#else
    b verify
#endif

.section .hardfault,"ax",%progbits
.thumb_func
hard_fault:
#if CASE == 3 || CASE == 4
    b verify
#else
    b fail
#endif

.section .text,"ax",%progbits
verify:
    cmp r1, #0x11
    bne fail
    ldr r3, =0xa5a5a5a5
    cmp r2, r3
    bne fail
    ldr r4, [sp, #4]
    cmp r1, r4
    bne fail
    ldr r4, [sp, #8]
    cmp r2, r4
    bne fail
    mov r4, sp
#if CASE == 4
    ldr r5, =0xfc0
#elif CASE >= 7
    ldr r5, =0xf98
#else
    ldr r5, =0xfe0
#endif
    cmp r4, r5
    bne fail
    ldr r11, [sp, #24]
#if CASE == 4
    movw r5, #0x300
#else
    movw r5, #0x200
#endif
    cmp r11, r5
    bne fail
#if CASE == 5 || CASE == 6
    cmp r10, r11
    bne fail
#endif
    ldr r3, =SCB_CFSR
    ldr r4, [r3]
    movw r5, #0x8200
    cmp r4, r5
    bne fail
    ldr r3, =SCB_BFAR
    ldr r4, [r3]
    cmp r4, r0
    bne fail
    ldr r3, =SCB_HFSR
    ldr r4, [r3]
#if CASE == 3 || CASE == 4
    cmp.w r4, #0x40000000
#else
    cmp r4, #0
#endif
    bne fail
#if CASE == 3
    str r4, [r3]
    ldr r4, [r3]
    cmp r4, #0
    bne fail
#endif
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
success: .asciz "BusFault OK\n"
failure: .asciz "BusFault FAIL\n"
