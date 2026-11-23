.section .text.init
.globl _start
_start:
    li t0, 0x600
    csrw mstatus, t0
    li a0, -1
    csrw vstart, a0
    csrr a0, vstart
    li t0, 255
    bne a0, t0, failed
    csrr a0, vlenb
    li t0, 32
    bne a0, t0, failed
    vsetvli a0, zero, e8, m2, ta, ma
    li t0, 64
    bne a0, t0, failed
    csrr a0, vstart
    bnez a0, failed
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
