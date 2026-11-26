.section .text.init
.globl _start
_start:
#ifdef HARTID
    csrr a0, mhartid
#elif defined(DOUBLE)
    amoadd.d a2, a1, (a0)
    amoadd.d a2, a1, (a0)
#else
    amoadd.w a2, a1, (a0)
    amoadd.w a2, a1, (a0)
#endif
