.option norvc
.text
.global _start
_start:
    csrr t0, misa
    li t1, 1 << 12
    and t0, t0, t1
    bnez t0, fail
    la t0, trap
    csrw mtvec, t0
    li a0, 12
    li a1, 3
    mul a2, a0, a1
    li t0, 36
    bne a2, t0, fail
    li a3, 0
    la a4, div_op
div_op:
    div a2, a0, a1
    la a4, rem_op
rem_op:
    rem a2, a0, a1
    li t0, 2
    bne a3, t0, fail
    li t0, 36
    bne a2, t0, fail
    li t0, 1
    j done
trap:
    csrr t0, mcause
    li t1, 2
    bne t0, t1, fail
    csrr t0, mepc
    bne t0, a4, fail
    addi t0, t0, 4
    csrw mepc, t0
    addi a3, a3, 1
    mret
fail:
    li t0, 3
done:
    li t1, 0x80001000
    sw t0, 0(t1)
    j .
