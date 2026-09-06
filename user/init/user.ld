/* Aether /init — static non-PIE ELF64, linked at 32 MiB. */
ENTRY(_start)

SECTIONS
{
    . = 0x2000000;

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
    }

    .bss : {
        *(COMMON)
        *(.bss .bss.*)
    }

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment)
        *(.gcc_except_table*)
        *(.interp)
    }
}
