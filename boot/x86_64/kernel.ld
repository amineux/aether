/* Aether kernel — higher-half VMA, physical LMA 4 MiB, PIC + .rela.dyn.
 *
 * Linked VA = 0xffffffff80000000 + PA  (classic x86_64 -2 GiB map).
 * The trampoline copies the flat binary to 0x400000, dual-maps an
 * 8 MiB kernel span at +KASLR slide (0 / 16 MiB / 32 MiB), applies
 * R_*_RELATIVE from the packed reloc table, unmaps the unused
 * canonical alias when slide != 0, and jumps to
 * 0xffffffff80400000+slide. Identity 4 GiB stays mapped for SoftNPU
 * DMA, page-table walks, AP SIPI, Multiboot mailbox, and user ELF
 * windows. Keep .rela.dyn (not discarded) so the slide can be a
 * real secret once the unused alias is gone.
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

    /* Dynamic relocs for the boot-time slide. Placed before BSS so
     * objcopy -O binary keeps the table in kernel.bin. */
    .rela.dyn : AT(ADDR(.rela.dyn) - KERNEL_OFFSET) {
        __rela_dyn_start = .;
        *(.rela.dyn)
        *(.rela.*)
        __rela_dyn_end = .;
    }

    .dynamic : AT(ADDR(.dynamic) - KERNEL_OFFSET) {
        *(.dynamic)
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
        *(.gnu.hash)
        *(.hash)
        *(.dynsym)
        *(.dynstr)
    }
}
