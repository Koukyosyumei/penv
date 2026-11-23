.section .text.init
.globl _start
_start:
    li t0, WINDOW
    ld a0, 0(t0)
    li t1, 0xfeedfacecafebeef
    li t2, 1
    beq a0, t1, finish
    li t2, 3
finish:
    li t0, 0x80010000
    sd t2, 0(t0)
1:  j 1b
