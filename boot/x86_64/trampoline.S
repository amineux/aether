/* Aether multiboot1 trampoline: 32-bit PM → long mode → kernel @ 4 MiB.
 *
 * QEMU -kernel loads this ELF32 with paging off. We identity-map 4 GiB
 * with 2 MiB pages, copy the embedded kernel blob to 0x400000, and jump.
 */
    .set ALIGN,    1
    .set MEMINFO,  2
    .set MAGIC,    0x1BADB002
    .set FLAGS,    (ALIGN | MEMINFO)
    .set CHECKSUM, -(MAGIC + FLAGS)

    .section .multiboot, "a"
    .align 4
    .long MAGIC
    .long FLAGS
    .long CHECKSUM

    .section .text
    .code32
    .global _start
_start:
    cli
    movl    $0x80000, %esp

    /* COM1 115200 8N1 so we can see boot progress if the kernel never runs. */
    movw    $0x3F9, %dx
    xorb    %al, %al
    outb    %al, %dx
    movw    $0x3FB, %dx
    movb    $0x80, %al
    outb    %al, %dx
    movw    $0x3F8, %dx
    movb    $1, %al
    outb    %al, %dx
    movw    $0x3F9, %dx
    xorb    %al, %al
    outb    %al, %dx
    movw    $0x3FB, %dx
    movb    $0x03, %al
    outb    %al, %dx
    movw    $0x3FA, %dx
    movb    $0xC7, %al
    outb    %al, %dx
    movw    $0x3FC, %dx
    movb    $0x0B, %al
    outb    %al, %dx

    movl    $boot_msg, %esi
    call    puts32

    /* Zero PML4/PDPT/PD at 0x1000..0x7000 */
    xorl    %eax, %eax
    movl    $0x1000, %edi
    movl    $(0x6000 / 4), %ecx
    rep     stosl

    movl    $0x2003, 0x1000          /* PML4[0] → PDPT */
    movl    $0x3003, 0x2000          /* PDPT[0] → PD0  0-1 GiB */
    movl    $0x4003, 0x2008          /* PDPT[1] → PD1 */
    movl    $0x5003, 0x2010
    movl    $0x6003, 0x2018

    /* 2048 × 2 MiB pages = 4 GiB identity map, PS|RW|P */
    movl    $0x3000, %edi
    movl    $0x00000083, %eax
    movl    $2048, %ecx
.fill_pd:
    movl    %eax, (%edi)
    movl    $0, 4(%edi)
    addl    $0x200000, %eax
    addl    $8, %edi
    loop    .fill_pd

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
    orl     $0x80000001, %eax        /* PG | PE */
    movl    %eax, %cr0

    lgdt    gdt_desc
    ljmp    $0x08, $long_mode

    .code64
long_mode:
    movw    $0x10, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss
    movw    %ax, %fs
    movw    %ax, %gs
    movl    $0x80000, %esp

    /* 32-bit immediates: this object is ELF32; RIP-relative is not representable. */
    movl    $lm_msg, %esi
    call    puts64

    /* Copy embedded kernel to 4 MiB. */
    movl    $payload_start, %esi
    movl    $payload_end, %ecx
    subl    %esi, %ecx
    movl    $0x400000, %edi
    cld
    rep     movsb

    movl    $jump_msg, %esi
    call    puts64

    movl    $0x400000, %eax
    jmp     *%rax

/* --- tiny polling UART (32-bit) --- */
    .code32
puts32:
    pushl   %eax
    pushl   %edx
1:
    movb    (%esi), %al
    testb   %al, %al
    jz      2f
    call    putc32
    incl    %esi
    jmp     1b
2:
    popl    %edx
    popl    %eax
    ret

putc32:
    pushl   %edx
    movw    $0x3FD, %dx
3:
    inb     %dx, %al
    testb   $0x20, %al
    jz      3b
    movw    $0x3F8, %dx
    movb    (%esi), %al
    outb    %al, %dx
    popl    %edx
    ret

    .code64
puts64:
    pushq   %rax
    pushq   %rdx
1:
    movb    (%rsi), %al
    testb   %al, %al
    jz      2f
    movw    $0x3FD, %dx
3:
    inb     %dx, %al
    testb   $0x20, %al
    jz      3b
    movw    $0x3F8, %dx
    movb    (%rsi), %al
    outb    %al, %dx
    incq    %rsi
    jmp     1b
2:
    popq    %rdx
    popq    %rax
    ret

    .align 8
gdt:
    .quad   0x0000000000000000
    .quad   0x00AF9A000000FFFF       /* 64-bit code */
    .quad   0x00AF92000000FFFF       /* 64-bit data */
gdt_end:
gdt_desc:
    .word   gdt_end - gdt - 1
    .long   gdt

boot_msg:
    .asciz  "[boot] Aether trampoline (multiboot1, 32-bit)\r\n"
lm_msg:
    .asciz  "[boot] long mode, copying kernel to 0x400000\r\n"
jump_msg:
    .asciz  "[boot] jumping to kmain\r\n"

    .section .payload, "a"
    .align  4096
    .global payload_start
payload_start:
    .incbin "build/kernel.bin"
    .global payload_end
payload_end:
