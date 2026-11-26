.section .text.init
.globl _start
_start:
    li a0, -1
    csrw menvcfg, a0
    csrr a1, menvcfg
