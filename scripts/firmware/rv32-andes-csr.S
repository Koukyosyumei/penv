.option norvc
.text
.global _start
_start:
    la t0, trap
    csrw mtvec, t0
    csrr t0, 0xfc2
    li t1, 0x10020
    bne t0, t1, fail
    csrr t0, 0x7c7
    li t1, -1
    bne t0, t1, fail
    li t1, 0x12345678
    csrw 0x7c7, t1
    csrr t0, 0x7c7
    bne t0, t1, fail
    li t1, 255
    csrw 0x7c6, t1
    csrr t0, 0x7c6
    li t1, 63
    bne t0, t1, fail
    li t1, 1
    csrw 0xbc0, t1
    csrr t0, 0xbc0
    bnez t0, fail

    li a2, 0
    li a3, 0x80002001
    la a4, misaligned
misaligned:
    lw t0, 0(a3)
    li t0, 1
    bne a2, t0, fail
    li t0, 64
    csrw 0x7d0, t0
    csrr t1, 0x7d0
    bne t0, t1, fail
    li t0, 0x12345678
    sw t0, 0(a3)
    lw t1, 0(a3)
    bne t0, t1, fail

    li a0, 0x1000
    li a1, 3
    .word 0x0ab5065b  # nds.lea.h a2, a0, a1
    li t0, 0x1006
    bne t0, a2, fail
    li t0, 1
    j done
trap:
    csrr t0, mcause
    li t1, 4
    bne t0, t1, fail
    csrr t0, mepc
    bne t0, a4, fail
    addi t0, t0, 4
    csrw mepc, t0
    addi a2, a2, 1
    mret
fail:
    li t0, 3
done:
    li t1, 0x80001000
    sw t0, 0(t1)
    j .
