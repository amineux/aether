/* Multiboot1 ELF32 loader. QEMU -kernel loads this at 1 MiB. */
OUTPUT_FORMAT(elf32-i386)
OUTPUT_ARCH(i386)
ENTRY(_start)

SECTIONS
{
    . = 1M;

    .multiboot : {
        *(.multiboot)
    }

    .text : {
        *(.text)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    .payload : ALIGN(4096) {
        *(.payload)
    }

    .data : {
        *(.data)
    }

    .bss : ALIGN(16) {
        *(.bss)
        *(COMMON)
    }
}
