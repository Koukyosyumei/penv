.section .text.init
.globl _start
_start:
    li a0, 0x600
    csrs mstatus, a0
    li a0, 127
    li a1, 127
    csrw vcsr, zero
    vsetvli t0, zero, e8, m1, ta, ma
    vmv.v.x v0, a0
    vmv.v.x v1, a1
    vsadd.vv v2, v0, v1
    csrr a0, vcsr
    li a1, 1
    bne a0, a1, failed
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
