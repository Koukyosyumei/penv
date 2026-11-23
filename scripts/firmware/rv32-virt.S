.section .text
.globl _start
_start:
    la a0, uart_ready
    call puts
    la t0, timer_irq
    csrw mtvec, t0
    li s0, 0x0200bff8           # mtime
    li s1, 0x02004000           # mtimecmp for hart 0
    li s2, 0
    lw t0, 0(s0)
    csrr s3, mcycle
    addi t0, t0, 64
    sw t0, 0(s1)
    sw zero, 4(s1)
    li t0, 128
    csrs mie, t0
    csrsi mstatus, 8
1:
    beqz s2, 1b
    la a0, timer_ready
    call puts
1:  j 1b

.balign 4
timer_irq:
    csrr t0, mcause
    li t1, 0x80000007
    bne t0, t1, fail
    csrr t0, mcycle
    sub t0, t0, s3
    li t1, 630
    bltu t0, t1, fail
    li t1, 660
    bgtu t0, t1, fail
    li t0, -1
    sw t0, 4(s1)
    li s2, 1
    mret

fail:
    la a0, bad_timer
    call puts
1:  j 1b

puts:
    li t0, 0x10000000           # NS16550 THR
1:
    lbu t1, 0(a0)
    beqz t1, 2f
    sb t1, 0(t0)
    addi a0, a0, 1
    j 1b
2:
    ret

.section .rodata
uart_ready: .asciz "rv32-virt: NS16550 UART ready\n"
timer_ready: .asciz "rv32-virt: 10 MHz CLINT machine timer IRQ and mret OK\n"
bad_timer: .asciz "rv32-virt: unexpected timer interrupt or frequency\n"
