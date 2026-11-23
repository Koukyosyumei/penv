ENTRY(_start)
PHDRS { firmware PT_LOAD FLAGS(5); }
SECTIONS {
    . = 0x80000000;
    .text : { *(.text*) *(.rodata*) } :firmware
}
