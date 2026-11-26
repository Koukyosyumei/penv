.section .text.init
.globl _start
_start:
#ifdef MSTATUS
    csrwi vcsr, 3
#else
    li a0, 0xff
    csrw EPC, a0
#endif
