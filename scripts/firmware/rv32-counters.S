.option norvc
.section .text
.globl _start

# Adapt the six RV32 MCYCLE/MINSTRET cases in Renode riscv-priv.robot.
.macro check_counter csr, mask
    csrw mcountinhibit, zero
    csrr a1, \csr
    nop
    csrr a2, \csr
    sub a3, a2, a1
    li t1, 2
    bne a3, t1, failure

    li a0, \mask
    csrw mcountinhibit, a0
    csrr a1, \csr
    nop
    csrr a2, \csr
    bne a1, a2, failure

    csrw mcountinhibit, zero
    csrr a1, \csr
    csrw mcountinhibit, a0
    nop
    csrw mcountinhibit, zero
    csrr a2, \csr
    sub a3, a2, a1
    bne a3, t1, failure
.endm

_start:
    check_counter mcycle, 1
    check_counter minstret, 4
    la a0, pass_message
    j puts
failure:
    la a0, fail_message
puts:
    li t0, 0x10000000          # NS16550 THR
1:
    lbu t1, 0(a0)
    beqz t1, 2f
    sb t1, 0(t0)
    addi a0, a0, 1
    j 1b
2:  j 2b

.section .rodata
pass_message: .asciz "rv32-counters: six MCYCLE/MINSTRET cases passed\n"
fail_message: .asciz "rv32-counters: unexpected counter value\n"
