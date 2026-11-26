.section .text
.globl _start
_start:
    la t0, trap
    csrw mtvec, t0
    la t0, supervisor
    csrw mepc, t0
    li t0, 0x00400800           # TSR=1, MPP=S
    csrw mstatus, t0
    li t0, -1
    csrw pmpaddr0, t0
    li t0, 0x1f
    csrw pmpcfg0, t0
    la t0, unexpected_return
    csrw sepc, t0
    mret

supervisor:
    sret

unexpected_return:
    ecall

.balign 4
trap:
    csrr t0, mcause
    li t1, 2
    bne t0, t1, failure
    csrr t0, mepc
    la t1, supervisor
    bne t0, t1, failure
    la a0, pass_message
    j puts
failure:
    la a0, fail_message
puts:
    li t0, 0x10000000
1:
    lbu t1, 0(a0)
    beqz t1, 2f
    sb t1, 0(t0)
    addi a0, a0, 1
    j 1b
2:  j 2b

.section .rodata
pass_message: .asciz "rv32-priv: SRET with TSR trapped at the expected PC\n"
fail_message: .asciz "rv32-priv: SRET with TSR did not trap as expected\n"
