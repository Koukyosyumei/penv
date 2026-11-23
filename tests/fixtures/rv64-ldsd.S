.section .text.init
.globl _start
_start:
#ifdef STORE
    .word 0x00a5b2a3
#else
    .word 0x0055b503
#endif
