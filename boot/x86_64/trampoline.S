/* Aether multiboot1 trampoline: 32-bit PM → long mode → higher-half kernel.
 *
 * QEMU -kernel loads this ELF32 with paging off. We stash the Multiboot
 * EAX/EBX mailbox at 0x7000 so the kernel can parse the mmap, identity-
 * map 4 GiB with 2 MiB pages, alias the first 2 GiB at the classic
 * -2 GiB kernel map (PML4[511] → same PDPT; dedicated HH PDs at
 * 0x71000/0x72000 so identity PDs never move), copy the embedded
 * kernel blob to LMA 0x400000, pick a KASLR slide (cmdline kaslr=0|1|2
 * or rdrand/rdtsc), dual-map an 8 MiB kernel span at +slide, and jump
 * to VA 0xffffffff80400000+slide. Identity stays for DMA / SIPI / user
 * windows. The canonical HH alias stays so code-model=kernel absolute
 * symbols still resolve. Not PIE, not KPTI, not PCID, not COW.
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
    /* Mailbox at 0x7000 (after boot PDs, before AP SIPI @ 0x8000).
     * EAX = Multiboot magic, EBX = info physical address, +8 = slide. */
    movl    %eax, 0x7000
    movl    %ebx, 0x7004
    movl    $0, 0x7008
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

    /* Higher-half alias on dedicated PDs (0x71000 / 0x72000) so a later
     * KASLR dual-map cannot rewrite identity leaves. Same PDPT as the
     * low map (entries 0..3 stay identity). */
    cld
    movl    $0x3000, %esi
    movl    $0x71000, %edi
    movl    $1024, %ecx
    rep     movsl
    movl    $0x4000, %esi
    movl    $0x72000, %edi
    movl    $1024, %ecx
    rep     movsl
    movl    $0x2003, 0x1FF8          /* PML4[511] → PDPT @ 0x2000 */
    movl    $0, 0x1FFC
    movl    $0x71003, 0x2FF0         /* PDPT[510] → HH PD0 (phys 0-1 GiB) */
    movl    $0, 0x2FF4
    movl    $0x72003, 0x2FF8         /* PDPT[511] → HH PD1 (phys 1-2 GiB) */
    movl    $0, 0x2FFC

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

    /* Copy embedded kernel to LMA 4 MiB (HH VA is 0xffffffff80400000+slide). */
    movl    $payload_start, %esi
    movl    $payload_end, %ecx
    subl    %esi, %ecx
    movl    $0x400000, %edi
    cld
    rep     movsb

    call    pick_kaslr
    movl    %eax, %ecx
    shll    $24, %ecx                 /* index * 16 MiB */
    movl    $0x7008, %ebx             /* 64-bit: no RIP-relative mailbox */
    movl    %ecx, (%ebx)
    call    apply_kaslr_alias

    movl    $jump_msg, %esi
    call    puts64

    /* as --32 will not emit a sign-extended movq imm32; movabs is
     * written as bytes so we land at 0xffffffff80400000 + slide. */
    .byte   0x48, 0xb8, 0x00, 0x00, 0x40, 0x80, 0xff, 0xff, 0xff, 0xff
    movl    $0x7008, %edx
    movl    (%edx), %edx
    addq    %rdx, %rax
    jmp     *%rax

/* --- KASLR: cmdline kaslr=0|1|2|off, else rdrand/rdtsc % 3 --- */
pick_kaslr:
    movl    $0x7000, %ebx
    movl    (%ebx), %eax
    cmpl    $0x2BADB002, %eax
    jne     .kaslr_entropy
    movl    4(%ebx), %ebx             /* info PA */
    testl   %ebx, %ebx
    jz      .kaslr_entropy
    movl    (%ebx), %eax              /* flags */
    testl   $4, %eax                  /* MB1_FLAG_CMDLINE */
    jz      .kaslr_entropy
    movl    16(%ebx), %esi            /* cmdline PA */
    testl   %esi, %esi
    jz      .kaslr_entropy
    movl    $256, %ecx                /* bound: no NUL → entropy */
.kaslr_scan:
    testl   %ecx, %ecx
    jz      .kaslr_entropy
    decl    %ecx
    movb    (%esi), %al
    testb   %al, %al
    jz      .kaslr_entropy
    cmpb    $'k', %al
    jne     .kaslr_next
    cmpb    $'a', 1(%esi)
    jne     .kaslr_next
    cmpb    $'s', 2(%esi)
    jne     .kaslr_next
    cmpb    $'l', 3(%esi)
    jne     .kaslr_next
    cmpb    $'r', 4(%esi)
    jne     .kaslr_next
    cmpb    $'=', 5(%esi)
    jne     .kaslr_next
    movb    6(%esi), %al
    cmpb    $'0', %al
    je      .kaslr_i0
    cmpb    $'1', %al
    je      .kaslr_i1
    cmpb    $'2', %al
    je      .kaslr_i2
    cmpb    $'o', %al
    jne     .kaslr_entropy
    cmpb    $'f', 7(%esi)
    jne     .kaslr_entropy
    cmpb    $'f', 8(%esi)
    jne     .kaslr_entropy
.kaslr_i0:
    xorl    %eax, %eax
    ret
.kaslr_i1:
    movl    $1, %eax
    ret
.kaslr_i2:
    movl    $2, %eax
    ret
.kaslr_next:
    incl    %esi
    jmp     .kaslr_scan
.kaslr_entropy:
    /* RDRAND #UDs if CPUID.1:ECX[30] is clear (stock qemu64). */
    pushq   %rbx
    movl    $1, %eax
    xorl    %ecx, %ecx
    cpuid
    testl   $(1 << 30), %ecx
    popq    %rbx
    jz      .kaslr_tsc
    .byte   0x0f, 0xc7, 0xf0          /* rdrand %eax */
    jc      .kaslr_mod3
.kaslr_tsc:
    rdtsc
.kaslr_mod3:
    andl    $3, %eax
    cmpl    $3, %eax
    jne     .kaslr_done
    xorl    %eax, %eax
.kaslr_done:
    ret

/* Dual-map 8 MiB of kernel pages at +slide on HH PD0 only. */
apply_kaslr_alias:
    movl    $0x7008, %ebx
    movl    (%ebx), %eax
    shrl    $21, %eax                 /* slide in 2 MiB pages */
    testl   %eax, %eax
    jz      .kaslr_alias_done
    movl    $0x71000, %edi            /* HH PD0 */
    movl    $2, %ecx                  /* LMA 4 MiB = PD index 2 */
.kaslr_alias_loop:
    movl    %ecx, %edx
    addl    %eax, %edx
    movq    (%edi,%ecx,8), %rsi
    movq    %rsi, (%edi,%edx,8)
    incl    %ecx
    cmpl    $6, %ecx                  /* indices 2..5 = 8 MiB */
    jb      .kaslr_alias_loop
.kaslr_alias_done:
    ret

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
    .asciz  "[boot] long mode, copying kernel to LMA 0x400000\r\n"
jump_msg:
    .asciz  "[boot] jumping to slid higher-half _start\r\n"

    .section .payload, "a"
    .align  4096
    .global payload_start
payload_start:
    .incbin "build/kernel.bin"
    .global payload_end
payload_end:
