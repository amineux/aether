/* 16-bit AP trampoline. Copied to physical 0x8000; SIPI vector = 8.
 *
 * Must be position-independent after the copy: no RIP-relative memory
 * operands (those would still point at the kernel .text link address).
 * Mailbox (filled by the BSP before SIPI):
 *   0x8F00  u64  AP stack top
 *   0x8F08  u64  ap_entry
 *   0x8F10  u32  signature (AP writes 0xA5A50001 once in long mode)
 */
.set AP_TRAMP,      0x8000
.set AP_MAIL_STACK, 0x8F00
.set AP_MAIL_ENTRY, 0x8F08
.set AP_MAIL_SIG,   0x8F10

.section .text.ap_tramp, "ax"
.align 16
.global ap_trampoline_start
ap_trampoline_start:
.code16
    cli
    cld
    xorw    %ax, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss
    lgdtl   AP_TRAMP + (ap_gdt_desc - ap_trampoline_start)
    movl    %cr0, %eax
    orl     $1, %eax
    movl    %eax, %cr0
    ljmpl   $0x08, $AP_TRAMP + (ap_pm32 - ap_trampoline_start)

.code32
ap_pm32:
    movw    $0x10, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss
    /* Reuse the BSP identity map (PML4 @ 0x1000). */
    movl    $0x1000, %eax
    movl    %eax, %cr3
    movl    %cr4, %eax
    orl     $0x20, %eax              /* PAE */
    movl    %eax, %cr4
    movl    $0xC0000080, %ecx        /* EFER */
    rdmsr
    orl     $0x100, %eax             /* LME */
    wrmsr
    movl    %cr0, %eax
    orl     $0x80000000, %eax        /* PG */
    movl    %eax, %cr0
    ljmpl   $0x18, $AP_TRAMP + (ap_lm64 - ap_trampoline_start)

.code64
ap_lm64:
    movw    $0x20, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss
    movw    %ax, %fs
    movw    %ax, %gs
    /* Absolute 32-bit addresses — do not use labels here. */
    movl    $AP_MAIL_SIG, %ebx
    movl    $0xA5A50001, (%rbx)
    movl    $AP_MAIL_STACK, %ebx
    movq    (%rbx), %rsp
    movl    $AP_MAIL_ENTRY, %ebx
    movq    (%rbx), %rax
    xorq    %rbp, %rbp
    andq    $-16, %rsp
    jmpq    *%rax

.align 8
ap_gdt:
    .quad   0x0000000000000000
    .quad   0x00CF9A000000FFFF       /* 08: 32-bit code */
    .quad   0x00CF92000000FFFF       /* 10: 32-bit data */
    .quad   0x00209A0000000000       /* 18: 64-bit code */
    .quad   0x0000920000000000       /* 20: 64-bit data */
ap_gdt_desc:
    .word   ap_gdt_desc - ap_gdt - 1
    .long   AP_TRAMP + (ap_gdt - ap_trampoline_start)

.global ap_trampoline_end
ap_trampoline_end:
.code64
