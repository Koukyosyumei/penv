.option norvc
.equ SUPERVISOR_VA_DELTA, 0x40000000
.equ TEST_VA, 0x40400000
.equ LEAF_FLAGS, 0x4b           # V, R, X, A

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

    la t0, leaf
    srli t1, t0, 12
    slli t1, t1, 10
    ori t1, t1, 1
    la t2, test_root_entry
    sw t1, 0(t2)
    la t1, old_page
    srli t1, t1, 12
    slli t1, t1, 10
    ori t1, t1, LEAF_FLAGS
    sw t1, 0(t0)
    la s1, new_page
    srli s1, s1, 12
    slli s1, s1, 10
    ori s1, s1, LEAF_FLAGS

    li t1, SUPERVISOR_VA_DELTA
    sub s0, t0, t1             # writable S-mode alias of the leaf PTE
    li s2, TEST_VA
    la t0, root
    srli t0, t0, 12
    li t2, 0x80000000          # SATP.MODE=Sv32
    or t0, t0, t2
    csrw satp, t0
    sfence.vma
    li t0, 0x800               # MPP=S
    csrw mstatus, t0
    la t0, supervisor
    sub t0, t0, t1
    csrw mepc, t0
    mret

supervisor:
    lw t0, 8(s2)               # warm the data translation
    li t1, 17
    bne t0, t1, supervisor_failure
    jalr ra, 0(s2)             # warm the instruction translation
    li t1, 17
    bne a0, t1, supervisor_failure

    sw s1, 0(s0)               # replace the active leaf using a guest store
    sfence.vma x0, x0           # invalidate instruction and data translations
    lw t0, 8(s2)
    li t1, 29
    bne t0, t1, supervisor_failure
    jalr ra, 0(s2)
    li t1, 29
    bne a0, t1, supervisor_failure
    li a0, 1
    ecall

supervisor_failure:
    li a0, 0
    ecall

.balign 4
trap:
    csrr t0, mcause
    li t1, 9
    bne t0, t1, failure
    li t0, 1
    bne a0, t0, failure
    la a0, pass_message
    j puts
failure:
    la a0, fail_message
puts:
    li t0, 0x10000000          # NS16550 THR, physical address in M mode
1:
    lbu t1, 0(a0)
    beqz t1, 2f
    sb t1, 0(t0)
    addi a0, a0, 1
    j 1b
2:  j 2b

.section .rodata
pass_message: .asciz "rv32-tlb: SFENCE.VMA refreshed Sv32 data and instruction translations\n"
fail_message: .asciz "rv32-tlb: stale translation or unexpected trap\n"

.balign 4096
root:
    .zero 256 * 4
    .word 0x200000cf           # VA 0x40000000 -> PA 0x80000000, S-mode 4 MiB RWX
test_root_entry:
    .zero 767 * 4

.balign 4096
leaf:
    .zero 4096

.section .text
.balign 4096
old_page:
    li a0, 17
    ret
    .word 17

.balign 4096
new_page:
    li a0, 29
    ret
    .word 29
