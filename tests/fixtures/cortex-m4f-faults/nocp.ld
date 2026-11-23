ENTRY(reset)
SECTIONS {
    .vectors 0x10000 : { *(.vectors) }
}
