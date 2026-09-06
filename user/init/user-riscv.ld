/* Aether RISC-V /init — static non-PIE ELF64, linked at 0x82000000. */
ENTRY(_start)

SECTIONS
{
    . = 0x82000000;

    .text : {
        *(.text.boot)
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    .data : {
        *(.data .data.*)
        *(.got .got.*)
        *(.sdata .sdata.*)
    }

    .bss : {
        *(COMMON)
        *(.bss .bss.*)
        *(.sbss .sbss.*)
    }

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment)
        *(.gcc_except_table*)
        *(.interp)
    }
}
