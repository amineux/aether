/* Aether RISC-V trampoline: OpenSBI S-mode -> Sv39 identity map -> kmain.
 *
 * QEMU virt + default OpenSBI loads this ELF at 0x80200000 (a0=hartid,
 * a1=dtb). We park extra harts, identity-map 4 GiB with 1 GiB pages,
 * and call the Rust kmain. UART0 is 16550 MMIO at 0x10000000.
 */
    .section .text.boot, "ax"
    .align 2
    .globl _start
_start:
    csrw    sie, zero
    csrw    sip, zero
    bnez    a0, .park

    la      sp, boot_stack_top
    andi    sp, sp, -16

    la      a0, msg_boot
    call    uart_puts

    /* Zero the Sv39 root (4 KiB). */
    la      t0, boot_pagetable
    li      t1, 4096
.zero_pt:
    sd      zero, 0(t0)
    addi    t0, t0, 8
    addi    t1, t1, -8
    bnez    t1, .zero_pt

    /* Four 1 GiB identity leaves: V|R|W|X|G|A|D = 0xEF. */
    la      t0, boot_pagetable
    li      t2, 0xEF
    li      t3, 0
    li      t4, 4
.fill_pt:
    slli    t5, t3, 28
    or      t5, t5, t2
    slli    t6, t3, 3
    add     t6, t0, t6
    sd      t5, 0(t6)
    addi    t3, t3, 1
    blt     t3, t4, .fill_pt

    /* satp.MODE = Sv39 (8), satp.PPN = root >> 12 */
    la      t0, boot_pagetable
    srli    t0, t0, 12
    li      t1, 8
    slli    t1, t1, 60
    or      t0, t0, t1
    csrw    satp, t0
    sfence.vma
    fence   rw, rw

    la      a0, msg_sv39
    call    uart_puts

    call    kmain

.hang:
    wfi
    j       .hang

.park:
    wfi
    j       .park

/* Polling 16550 TX at UART0. a0 = NUL-terminated string. */
uart_puts:
    li      t0, 0x10000000
1:
    lbu     t1, 0(a0)
    beqz    t1, 3f
2:
    lbu     t2, 5(t0)
    andi    t2, t2, 0x20
    beqz    t2, 2b
    sb      t1, 0(t0)
    addi    a0, a0, 1
    j       1b
3:
    ret

    .section .rodata
msg_boot:
    .asciz  "[boot] Aether trampoline (riscv64, OpenSBI S-mode)\r\n"
msg_sv39:
    .asciz  "[boot] Sv39 identity 4 GiB, jumping to kmain\r\n"

    .section .bss
    .align  12
    .globl  boot_pagetable
boot_pagetable:
    .skip   4096

    .align  4
    /* run_boot_demo holds Fabric + two CapTables; cdt/parent grew Capability. */
    .skip   131072
    .globl  boot_stack_top
boot_stack_top:
