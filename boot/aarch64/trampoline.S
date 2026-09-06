/* Aether aarch64 trampoline: QEMU virt EL1/EL2 -> TTBR identity map -> kmain.
 *
 * QEMU -machine virt,gic-version=2 -cpu cortex-a72 -kernel ELF loads us at
 * 0x40080000 (x0=dtb). We drop to EL1 if needed, park extra PEs, identity-map
 * 4 GiB (1 GiB blocks), and call Rust kmain. PL011 UART is at 0x09000000.
 *
 * EL0 /init is entered later from kmain via eret. No GICv3, no FDT.
 */
    .section .text.boot, "ax"
    .align 2
    .globl _start
_start:
    /* Park anyone who is not affinity 0. */
    mrs     x1, mpidr_el1
    and     x1, x1, #0xff
    cbnz    x1, .park

    /* EL2 -> EL1 if QEMU handed us the hypervisor. */
    mrs     x1, CurrentEL
    cmp     x1, #0x8
    b.eq    .el2_to_el1
    cmp     x1, #0x4
    b.eq    .el1_entry
    b       .hang

.el2_to_el1:
    mov     x1, #(1 << 31)          /* HCR_EL2.RW = AArch64 EL1 */
    msr     hcr_el2, x1
    mrs     x1, cnthctl_el2
    orr     x1, x1, #3              /* EL1PCTEN | EL1PCEN */
    msr     cnthctl_el2, x1
    msr     cntvoff_el2, xzr
    mov     x1, #0x33ff
    msr     cptr_el2, x1
    msr     hstr_el2, xzr
    /* SCTLR_EL1 RES1 bits; MMU/caches off until we enable them. */
    ldr     x1, =0x30d00800
    msr     sctlr_el1, x1
    mov     x1, #0x3c5              /* EL1h, DAIF masked */
    msr     spsr_el2, x1
    adr     x1, .el1_entry
    msr     elr_el2, x1
    isb
    eret

.el1_entry:
    msr     daifset, #0xf
    /* Allow FP/SIMD so rustc memcpy does not UNDEF. */
    mov     x1, #(3 << 20)
    msr     cpacr_el1, x1
    isb

    ldr     x1, =boot_stack_top
    and     x1, x1, #~15
    mov     sp, x1

    adr     x0, msg_boot
    bl      uart_puts

    /* Zero the L1 table (512 x 8 = 4 KiB). */
    ldr     x0, =boot_pagetable
    mov     x1, #4096
.zero_pt:
    str     xzr, [x0], #8
    subs    x1, x1, #8
    b.ne    .zero_pt

    /* Four 1 GiB identity blocks.
     *   [0]: Device-nGnRnE (UART/GIC MMIO)
     *   [1]: Normal WB (RAM @ 0x40000000)
     *   [2],[3]: Device (upper MMIO / unused)
     * Block desc: valid | AF | UXN | AttrIndx | SH.
     */
    ldr     x0, =boot_pagetable
    /* Device: valid + AF + UXN */
    mov     x2, #(1 | (1 << 10))
    mov     x3, #(1 << 22)          /* UXN is bit 54; split for 32-bit immediates */
    /* UXN = bit 54 = 1 << 54. Build as (1 << 22) << 32. */
    lsl     x3, x3, #32
    orr     x2, x2, x3
    str     x2, [x0, #0]

    /* Normal @ 1 GiB: valid | AF | AttrIndx=1 | Inner-shareable | UXN */
    mov     x2, #(1 | (1 << 2) | (3 << 8) | (1 << 10))
    orr     x2, x2, x3
    mov     x4, #(1 << 30)          /* PA 0x40000000 */
    orr     x2, x2, x4
    str     x2, [x0, #8]

    /* Device @ 2 GiB and 3 GiB */
    mov     x2, #(1 | (1 << 10))
    orr     x2, x2, x3
    mov     x4, #(1 << 31)          /* PA 0x80000000 */
    orr     x2, x2, x4
    str     x2, [x0, #16]

    mov     x2, #(1 | (1 << 10))
    orr     x2, x2, x3
    mov     x4, #0xc0000000
    orr     x2, x2, x4
    str     x2, [x0, #24]

    /* MAIR: Attr0 = Device-nGnRnE, Attr1 = Normal WB. */
    mov     x1, #0x00
    mov     x2, #0xff
    orr     x1, x1, x2, lsl #8
    msr     mair_el1, x1

    /* TCR: T0SZ=25 (39-bit), 4K TG0, inner-shareable WB, EPD1, IPS=40-bit.
     * 0x2 << 32 | EPD1 | T1SZ=25 | SH0=ISH | ORGN0=WB | IRGN0=WB | T0SZ=25
     */
    ldr     x1, =0x0000000200993519
    msr     tcr_el1, x1

    ldr     x1, =boot_pagetable
    msr     ttbr0_el1, x1
    msr     ttbr1_el1, xzr
    dsb     sy
    isb
    tlbi    vmalle1
    dsb     sy
    isb

    /* SCTLR_EL1: M | C | I. Keep RES1. */
    mrs     x1, sctlr_el1
    orr     x1, x1, #(1 << 0)
    orr     x1, x1, #(1 << 2)
    orr     x1, x1, #(1 << 12)
    msr     sctlr_el1, x1
    isb

    adr     x0, msg_ttbr
    bl      uart_puts

    bl      kmain

.hang:
    wfi
    b       .hang

.park:
    wfi
    b       .park

/* Polling PL011 TX. x0 = NUL-terminated string. Clobbers x1–x3. */
uart_puts:
    ldr     x1, =0x09000000
1:
    ldrb    w2, [x0]
    cbz     w2, 3f
2:
    ldr     w3, [x1, #0x18]
    tbnz    w3, #5, 2b              /* TXFF */
    str     w2, [x1]
    add     x0, x0, #1
    b       1b
3:
    ret

    .section .rodata
msg_boot:
    .asciz  "[boot] Aether trampoline (aarch64, QEMU virt EL1)\r\n"
msg_ttbr:
    .asciz  "[boot] TTBR identity 4 GiB, jumping to kmain\r\n"

    .section .bss
    .align  12
    .globl  boot_pagetable
boot_pagetable:
    .skip   4096

    .align  4
    /* CDT-sized Fabric overflowed 64/128 KiB on x86; match that stack. */
    .skip   262144
    .globl  boot_stack_top
boot_stack_top:
