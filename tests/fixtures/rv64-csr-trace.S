.section .text.init
.globl _start
_start:
#if defined(IMMEDIATE)
    csrrwi x0, 0x7c0, 13
#elif defined(REGISTER)
    csrrw x0, 0x7c0, a0
#else
    csrrsi x0, 0x7c0, 1
#endif
