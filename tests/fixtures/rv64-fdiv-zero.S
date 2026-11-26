.section .text.init
.globl _start
_start:
#ifdef HALF
    fdiv.h f2, f1, f0
#else
    fdiv.s f2, f1, f0
#endif
