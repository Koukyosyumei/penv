.section .text.init
.globl _start
_start:
    la s0, data
    li s1, 0x80
    c.sb s1, 1(s0)
    c.lbu a0, 1(s0)
    bne a0, s1, failed
    li s1, 0x8002
    c.sh s1, 2(s0)
    c.lhu a0, 2(s0)
    bne a0, s1, failed
    c.lh a0, 2(s0)
    li t0, -32766
    bne a0, t0, failed

    li a0, -128
    c.zext.b a0
    li t0, 128
    bne a0, t0, failed
    c.sext.b a0
    li t0, -128
    bne a0, t0, failed
    li a0, -32766
    c.zext.h a0
    bne a0, s1, failed
    c.sext.h a0
    li t0, -32766
    bne a0, t0, failed
    li a0, -2
    c.zext.w a0
    li t0, 0xfffffffe
    bne a0, t0, failed
    li a0, -3
    c.not a0
    li t0, 2
    bne a0, t0, failed
    li a1, 3
    c.mul a0, a1
    li t0, 6
    bne a0, t0, failed
    li a0, 1
    j finish
failed:
    li a0, 3
finish:
    la t0, tohost
    sd a0, 0(t0)
1:  j 1b

.section .tohost,"aw"
.balign 4096
.globl tohost
tohost:
    .dword 0
.section .data
data:
    .word 0
