/* Aether aarch64 kernel — EL1 payload at 0x40200000 (QEMU virt).
 * 2 MiB above RAM base so QEMU can put its 1 MiB DTB at 0x40000000. */
OUTPUT_ARCH(aarch64)
ENTRY(_start)

SECTIONS
{
    . = 0x40200000;

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
