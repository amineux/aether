/* Aether RISC-V kernel — S-mode payload at 0x80200000 (OpenSBI). */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x80200000;

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
