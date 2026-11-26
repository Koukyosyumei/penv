.option norvc
.option norelax
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
    call puts

    # RV32 adaptation of Renode's "Should Fail On Counter Access" (RV64).
    la t0, counter_trap
    csrw mtvec, t0
    csrw medeleg, zero
    csrw mcounteren, zero
    li t0, -1
    csrw pmpaddr0, t0
    li t0, 0x1f
    csrw pmpcfg0, t0
    li s0, 0
    la t0, supervisor
    csrw mepc, t0
    li t0, 0x800              # MPP=S
    csrw mstatus, t0
    mret

supervisor:
    li t0, 7
    csrw scounteren, t0
    la t0, user
    csrw sepc, t0
    sret

user:
    csrr a0, mcycle
    rdinstret a0
enable_counters:
    ecall
    rdinstret a0
complete:
    ecall

.balign 4
counter_trap:
    csrr t0, mcause
    li t1, 2
    beq t0, t1, denied_counter
    li t1, 8
    bne t0, t1, failure
    csrr t0, mepc
    la t1, enable_counters
    beq t0, t1, allow_counters
    la t1, complete
    bne t0, t1, failure
    li t1, 2
    bne s0, t1, failure
    la a0, privilege_message
    call puts
    j idle

allow_counters:
    li t1, 2
    bne s0, t1, failure
    csrsi mcounteren, 4
    j resume_user

denied_counter:
    li t0, 2
    bgeu s0, t0, failure
    csrr t0, mepc
    la t1, user
    slli t2, s0, 2
    add t1, t1, t2
    bne t0, t1, failure
    addi s0, s0, 1
resume_user:
    addi t0, t0, 4
    csrw mepc, t0
    mret

failure:
    la a0, fail_message
    call puts
idle:
    j idle

puts:
    li t0, 0x10000000          # NS16550 THR
1:
    lbu t1, 0(a0)
    beqz t1, 2f
    sb t1, 0(t0)
    addi a0, a0, 1
    j 1b
2:  ret

.section .rodata
pass_message: .asciz "rv32-counters: six MCYCLE/MINSTRET cases passed\n"
privilege_message: .asciz "rv32-counters: U-mode counter permissions passed\n"
fail_message: .asciz "rv32-counters: unexpected counter value or trap\n"
