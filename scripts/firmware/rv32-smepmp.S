    .section .text
    .option norvc
    .globl _start

# RV64 c688990's three physical access inputs, adapted to RV32 lw/sw.
_start:
    la t0, trap
    csrw mtvec, t0
    li t0, -1
    csrw 0x757, t0
    csrr t0, 0x757
    bnez t0, fail

    csrwi 0x747, 4             # RLB permits configuring locked executable code.
    li t0, 0x200001ff          # NAPOT code 0x80000000..0x80000fff
    csrw pmpaddr0, t0
    li t0, 0x20000401          # NAPOT shared data 0x80001000..0x8000100f
    csrw pmpaddr1, t0
    li t0, 0x04000000          # NA4 UART TX at 0x10000000
    csrw pmpaddr2, t0
    li t0, 0x0093009d          # Install the executable rule before enabling MML.
    csrw pmpcfg0, t0
    csrwi 0x747, 5
    li t0, 0x00931e9d          # M-only RX code, shared RW data, M-only RW UART
    csrw pmpcfg0, t0
    li a0, 0x10000000
    csrwi 0x747, 3             # Set MML and MMWP, clear RLB.
    csrwi 0x747, 4             # Locked entries prevent re-enabling RLB.
    csrr t0, 0x747
    li t1, 3
    bne t0, t1, fail

    li t0, 0x80001000
    li t1, 0x65657a            # Data value used by upstream Renode pmp.robot.
    sw t1, 0(t0)
    lw t2, 0(t0)
    bne t1, t2, fail

    li s0, 0x80001100          # Mapped RAM, outside all PMP entries.
    li s1, 5
    la s2, after_load
    la s3, denied_load
denied_load:
    lw t0, 0(s0)
    j fail
after_load:
    li s1, 7
    la s2, after_store
    la s3, denied_store
denied_store:
    sw zero, 0(s0)
    j fail
after_store:
    li s1, 1
    la s2, passed
    mv s3, s0
    jr s0

trap:
    csrr t0, mcause
    bne t0, s1, fail
    csrr t0, mtval
    bne t0, s0, fail
    csrr t0, mepc
    bne t0, s3, fail
    csrw mepc, s2
    mret

passed:
    la t0, pass_message
    j print
fail:
    li a0, 0x10000000
    la t0, fail_message
print:
    lbu t1, 0(t0)
    beqz t1, halt
    sb t1, 0(a0)
    addi t0, t0, 1
    j print
halt:
    j halt

pass_message: .asciz "rv32-smepmp: allowed RW, denied R/W/X with causes 5/7/1\n"
fail_message: .asciz "rv32-smepmp: unexpected access or trap\n"
