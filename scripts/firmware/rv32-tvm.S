.option norvc
.equ TVM, 0x00100000
.equ MPP_MASK, 0x1800
.equ MPP_S, 0x0800
.equ SATP_SV32, 0x80000000
.equ SATP_ASID1, 0x00400000
.equ SUPERVISOR_VA_DELTA, 0x40000000

.section .text
.globl _start
_start:
    la t0, trap
    csrw mtvec, t0
    csrw medeleg, zero
    li t0, -1
    csrw pmpaddr0, t0
    li t0, 0x1f
    csrw pmpcfg0, t0

    li t0, TVM | MPP_S
    csrw mstatus, t0
    la s2, root
    srli s2, s2, 12
    li t0, SATP_SV32
    or s2, s2, t0
    li t0, SATP_ASID1
    or s3, s2, t0
    csrw satp, s2               # TVM does not restrict M mode
    sfence.vma
    csrr t0, satp
    bne t0, s2, failure

    li s0, 0                    # phase: TVM enabled
    li s1, 0                    # count/order of blocked S-mode instructions
    li s4, SUPERVISOR_VA_DELTA
enter_supervisor:
    la t0, supervisor
    sub t0, t0, s4
    csrw mepc, t0
    mret

supervisor:
    li a0, -1
blocked_read:
    csrr a0, satp
blocked_write:
    csrw satp, s3
blocked_fence:
    sfence.vma
    beqz s0, supervisor_done
    bne a0, s2, supervisor_failed
    csrr a0, satp
    bne a0, s3, supervisor_failed
supervisor_done:
    ecall
supervisor_failed:
    ebreak

.balign 4
trap:
    csrr t0, mstatus
    li t1, MPP_MASK
    and t0, t0, t1
    li t1, MPP_S
    bne t0, t1, failure        # every trap must originate in S mode
    csrr t0, mcause
    li t1, 9
    beq t0, t1, phase_done
    li t1, 2
    bne t0, t1, failure
    bnez s0, failure
    csrr t0, mstatus
    li t1, TVM
    and t0, t0, t1
    beqz t0, failure

    csrr t0, mepc
    la t1, blocked_read
    sub t1, t1, s4
    slli t2, s1, 2
    add t1, t1, t2            # expect read, write, fence at consecutive PCs
    bne t0, t1, failure
    li t2, 3
    bgeu s1, t2, failure
    add t1, t0, s4
    lw t1, 0(t1)
    csrr t2, mtval
    bne t1, t2, failure
    addi s1, s1, 1
    addi t0, t0, 4
    csrw mepc, t0
    mret

phase_done:
    csrr t0, mepc
    la t1, supervisor_done
    sub t1, t1, s4
    bne t0, t1, failure
    li t0, 3
    bne s1, t0, failure
    csrr t0, satp
    bnez s0, second_phase_done
    bne t0, s2, failure
    li t0, -1
    bne a0, t0, failure       # denied CSR read did not alter its destination
    li s0, 1
    li t0, TVM
    csrc mstatus, t0
    j enter_supervisor

second_phase_done:
    bne t0, s3, failure        # S mode wrote the new ASID with TVM clear
    la a0, pass_message
    j puts
failure:
    la a0, fail_message
puts:
    li t0, 0x10000000          # NS16550 THR (physical, in M mode)
1:
    lbu t1, 0(a0)
    beqz t1, 2f
    sb t1, 0(t0)
    addi a0, a0, 1
    j 1b
2:  j 2b

.section .rodata
pass_message: .asciz "rv32-tvm: Sv32 supervisor SATP read/write and SFENCE.VMA gated; MRET recovery OK\n"
fail_message: .asciz "rv32-tvm: unexpected privilege transition or translation operation\n"

.balign 4096
root:
    .zero 256 * 4
    .word 0x200000cf           # VA 0x40000000 -> PA 0x80000000, S-mode 4 MiB leaf
    .zero 767 * 4
