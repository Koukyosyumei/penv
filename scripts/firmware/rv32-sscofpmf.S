.option norvc
.text
.global _start
_start:
    la t0, trap
    csrw mtvec, t0
    li a2, 0
    li t0, -1
    csrw 0x323, t0
    csrr t1, 0x323
    bnez t1, fail
#ifdef ENABLED
    csrw 0x723, t0
    csrr t1, 0x723
    bnez t1, fail
    csrw 0x73f, t0
    csrr t1, 0x73f
    bnez t1, fail
    csrr t1, 0xda0
    bnez t1, fail
    la a3, bad_write
bad_write:
    csrw 0xda0, t0
    li t0, 1
#else
    la a3, bad_high
bad_high:
    csrr t1, 0x723
    la a3, bad_overflow
bad_overflow:
    csrr t1, 0xda0
    li t0, 2
#endif
    bne a2, t0, fail
    li t0, 1
    j done
trap:
    csrr t0, mcause
    li t1, 2
    bne t0, t1, fail
    csrr t0, mepc
    bne t0, a3, fail
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
