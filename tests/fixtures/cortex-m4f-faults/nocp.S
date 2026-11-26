.syntax unified
.thumb
.fpu fpv4-sp-d16
.section .vectors,"a",%progbits
.word 0x1000, reset, 0, 0, 0, 0, usage_fault
.align 12
.global reset
.thumb_func
reset:
    ldr r1, =0xe000ed24
    ldr r2, [r1]
    orr r2, r2, #0x40000
    str r2, [r1]
#if CASE == 1
    ldr r1, =0xe000ed88
    ldr r2, =0xf00000
    str r2, [r1]
    dsb
    isb
#endif
    ldr r3, =0x100000
    mov.w r7, #0x200
    ldr r2, =0x10000
fault_instruction:
    vldr s15, [r2]
    vcvt.s32.f32 s15, s15
    vcvt.f32.s32 s15, s15
    vstr s15, [r7]
    ldr r4, =ok
    b print
.thumb_func
usage_fault:
    ldr r2, =0xe000ed2a
    ldrh r2, [r2]
    cmp r2, #8
    bne fail
    ldr r1, [sp, #24]
    ldr r2, =fault_instruction
    cmp r1, r2
    bne fail
    ldr r4, =nocp
    b print
fail:
    ldr r4, =failed
print:
    mov.w r3, #0x100000
1:  ldrb r0, [r4], #1
    cbz r0, 2f
    strb r0, [r3]
    b 1b
2:  bkpt #0
.ltorg
ok: .asciz "OK\n"
nocp: .asciz "NOCP\n"
failed: .asciz "FAIL\n"
