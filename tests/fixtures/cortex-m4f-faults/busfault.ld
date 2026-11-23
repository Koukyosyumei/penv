ENTRY(reset)
SECTIONS {
    .vectors 0 : { *(.vectors) }
    .boot 0x100 : { *(.boot) }
    .fault 0x200 : { *(.fault) }
    .busfault 0x300 : { *(.busfault) }
    .hardfault 0x340 : { *(.hardfault) }
    .text 0x400 : { *(.text) }
}
