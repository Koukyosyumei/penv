.option norvc
.text
.global _start
_start:
    csrr a0, misa
    andi a0, a0, 0x110
    li a1, 0x10
    bne a0, a1, fail
    csrrwi zero, mscratch, 31
    csrr a0, mscratch
    li a1, 31
    bne a0, a1, fail
    la a0, trap
    csrw mtvec, a0
    li a2, 0
    li a3, 0x00100813
    li a4, 4
    .word 0x00100813  # addi x16, x0, 1
    li a3, 0x00080513
    .word 0x00080513  # addi a0, x16, 0
    li a3, 0x4805
    li a4, 2
    .half 0x4805     # c.li x16, 1
    li a0, 3
    bne a2, a0, fail
    li a0, 1
    j done
.balign 4
trap:
    csrr a0, mcause
    li a1, 2
    bne a0, a1, fail
    csrr a0, mtval
    bne a0, a3, fail
    csrr a0, mepc
    add a0, a0, a4
    csrw mepc, a0
    addi a2, a2, 1
    mret
fail:
    li a0, 3
done:
    li a1, 0x80001000
    sw a0, 0(a1)
    j .
