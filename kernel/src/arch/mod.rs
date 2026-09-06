//! Architecture HAL. Only x86_64 is implemented; RISC-V / aarch64 hook here.

pub mod irq;
pub mod x86_64;

pub use x86_64::{gdt, idt, serial, syscall, timer};
