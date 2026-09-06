pub mod gdt;
pub mod idt;
pub mod io;
pub mod serial;
pub mod syscall;
pub mod timer;

pub fn console_name() -> &'static str {
    "COM1 115200"
}

pub fn idle() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

pub fn exit_qemu(success: bool) {
    io::outb(0xF4, if success { 0x00 } else { 0x01 });
}
