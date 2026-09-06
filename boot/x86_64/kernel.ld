/* Aether kernel — higher-half VMA, physical LMA 4 MiB.
 *
 * VA = 0xffffffff80000000 + PA  (classic x86_64 -2 GiB kernel map).
 * The trampoline copies the flat binary to 0x400000 and jumps to
 * 0xffffffff80400000 after installing PML4[511] (alias of the first
 * 2 GiB). The identity 4 GiB stays mapped for SoftNPU DMA, page-table
 * walks, AP SIPI, Multiboot mailbox, and user ELF windows.
 */
KERNEL_OFFSET = 0xFFFFFFFF80000000;
KERNEL_LMA    = 0x400000;

ENTRY(_start)

SECTIONS
{
    . = KERNEL_OFFSET + KERNEL_LMA;

    .text : AT(ADDR(.text) - KERNEL_OFFSET) {
        *(.text.boot)
        *(.text .text.*)
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_OFFSET) {
        *(.rodata .rodata.*)
    }

    .data : AT(ADDR(.data) - KERNEL_OFFSET) {
        *(.data .data.*)
        *(.got .got.*)
    }

    .bss : AT(ADDR(.bss) - KERNEL_OFFSET) {
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
