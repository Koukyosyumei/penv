    .section .text
    .option norvc
    .globl _start
_start:
    # Same initialization sequence as ACT tests/env/utils.h RVTEST_V_ENABLE.
    li t2, 0x200
    csrs mstatus, t2
    csrr t1, vlenb
    li t0, 128
    bne t1, t0, fail

    # Supervisor alias and the state after reset.
    csrr t0, sstatus
    andi t0, t0, 0x600
    bne t0, t2, fail
    csrr t0, vtype
    li t1, 0x80000000
    bne t0, t1, fail

    # vsetvli with rs1=x0 gets VLMAX. Native ELEN=64 and VLEN=1024.
    vsetvli t0, zero, e64, m1, ta, ma
    li t1, 16
    bne t0, t1, fail
    csrr t0, vl
    bne t0, t1, fail

    # Register AVL and vtype. Retune the type while preserving VLMAX.
    li t2, 7
    li t3, 0x10              # e32,m1,tu,mu
    vsetvl t0, t2, t3
    bne t0, t2, fail
    vsetvli zero, zero, e64, m2, ta, ma
    csrr t0, vl
    bne t0, t2, fail

    # vstart WARL, vsetivli clearing vstart, and vcsr aliases.
    li t3, -1
    csrw vstart, t3
    csrr t0, vstart
    li t1, 1023
    bne t0, t1, fail
    vsetivli t0, 3, e8, mf8, ta, ma
    li t1, 3
    bne t0, t1, fail
    csrr t0, vstart
    bnez t0, fail
    csrwi vxrm, 3
    csrwi vxsat, 1
    csrr t0, vcsr
    li t1, 7
    bne t0, t1, fail

    li t3, 0x100            # Reserved vtype bit.
    vsetvl t0, t2, t3
    bnez t0, fail
    csrr t0, vtype
    li t1, 0x80000000
    bne t0, t1, fail
    csrr t0, mstatus
    li t1, 0x80000600
    and t0, t0, t1
    bne t0, t1, fail

    la t0, pass_message
    j print
fail:
    la t0, fail_message
print:
    li a0, 0x10000000
1:  lbu t1, 0(t0)
    beqz t1, 2f
    sb t1, 0(a0)
    addi t0, t0, 1
    j 1b
2:  j 2b

pass_message: .asciz "rv32-vector-bootstrap: VS, vlenb, vsetvl and vector CSRs OK\n"
fail_message: .asciz "rv32-vector-bootstrap: unexpected vector state\n"
