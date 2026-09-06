//! Architecture HAL. x86_64 is the full ring-3 path; riscv64 is S-mode
//! + U-mode `/init` + PLIC SoftNPU doorbell; aarch64 is EL1 + EL0 `/init`.

pub mod irq;

#[cfg(target_arch = "x86_64")]
pub mod x86_64;
#[cfg(target_arch = "riscv64")]
pub mod riscv64;
#[cfg(target_arch = "aarch64")]
pub mod aarch64;

#[cfg(target_arch = "x86_64")]
pub use x86_64::{gdt, idt, serial, syscall, timer};
#[cfg(target_arch = "riscv64")]
pub use riscv64::{idt, serial, timer};
#[cfg(target_arch = "aarch64")]
pub use aarch64::{idt, serial, timer};

pub fn console_name() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        x86_64::console_name()
    }
    #[cfg(target_arch = "riscv64")]
    {
        riscv64::console_name()
    }
    #[cfg(target_arch = "aarch64")]
    {
        aarch64::console_name()
    }
}

pub fn idle() -> ! {
    #[cfg(target_arch = "x86_64")]
    {
        x86_64::idle()
    }
    #[cfg(target_arch = "riscv64")]
    {
        riscv64::idle()
    }
    #[cfg(target_arch = "aarch64")]
    {
        aarch64::idle()
    }
}

pub fn exit_qemu(success: bool) {
    #[cfg(target_arch = "x86_64")]
    {
        x86_64::exit_qemu(success)
    }
    #[cfg(target_arch = "riscv64")]
    {
        riscv64::exit_qemu(success)
    }
    #[cfg(target_arch = "aarch64")]
    {
        aarch64::exit_qemu(success)
    }
}

pub fn kernel_text_va() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        aether_core::KERNEL_TEXT_VA
    }
    #[cfg(target_arch = "riscv64")]
    {
        riscv64::KERNEL_VA
    }
    #[cfg(target_arch = "aarch64")]
    {
        aarch64::KERNEL_VA
    }
}

pub fn frame_window() -> (u64, u64) {
    #[cfg(target_arch = "x86_64")]
    {
        (0x0100_0000, 0x0800_0000)
    }
    #[cfg(target_arch = "riscv64")]
    {
        (riscv64::FRAME_START, riscv64::FRAME_END)
    }
    #[cfg(target_arch = "aarch64")]
    {
        (aarch64::FRAME_START, aarch64::FRAME_END)
    }
}

pub fn identity_map_note() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "[boot] higher-half + identity 4 GiB (2 MiB pages) from trampoline"
    }
    #[cfg(target_arch = "riscv64")]
    {
        "[boot] Sv39 identity map 4 GiB (1 GiB pages) from trampoline"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "[boot] TTBR0 identity map 4 GiB (1 GiB blocks) from trampoline; EL0 splits RAM to 2 MiB"
    }
}
