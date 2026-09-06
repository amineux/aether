# Aether — accelerator-first fabric kernel
# `make qemu` is the demo. `make test` is host-side logic.

TARGET      := x86_64-unknown-none
KERNEL_DIR  := kernel
KERNEL_ELF  := $(KERNEL_DIR)/target/$(TARGET)/release/aether
BUILD       := build
KERNEL_BIN  := $(BUILD)/kernel.bin
LOADER_ELF  := $(BUILD)/aether.elf
QEMU        := qemu-system-x86_64
QEMU_FLAGS  := -kernel $(LOADER_ELF) -serial stdio -display none \
               -no-reboot -no-shutdown -m 128M \
               -device isa-debug-exit,iobase=0xf4,iosize=0x04

.PHONY: all kernel loader qemu qemu-debug test test-host clean help

all: $(LOADER_ELF)

help:
	@echo "Aether targets:"
	@echo "  make test   - host unit tests (caps, fabric, arenas, sched, npu)"
	@echo "  make qemu   - build + boot the demo under QEMU (serial on stdio)"
	@echo "  make clean"

test: test-host

test-host:
	cargo test --workspace

kernel:
	cd $(KERNEL_DIR) && cargo build --release --target $(TARGET)

$(KERNEL_ELF): kernel

$(KERNEL_BIN): $(KERNEL_ELF)
	mkdir -p $(BUILD)
	objcopy -O binary $(KERNEL_ELF) $(KERNEL_BIN)
	@echo "kernel.bin $$(wc -c < $(KERNEL_BIN)) bytes"

$(BUILD)/trampoline.o: boot/x86_64/trampoline.S $(KERNEL_BIN) boot/x86_64/trampoline.ld
	as --32 -o $@ boot/x86_64/trampoline.S

$(LOADER_ELF): $(BUILD)/trampoline.o boot/x86_64/trampoline.ld
	ld -m elf_i386 -T boot/x86_64/trampoline.ld -o $(LOADER_ELF) $(BUILD)/trampoline.o
	@echo "loader $$(wc -c < $(LOADER_ELF)) bytes"

# isa-debug-exit: write 0 → qemu status 1. Treat that as a clean demo.
qemu: $(LOADER_ELF)
	$(QEMU) $(QEMU_FLAGS); \
	ec=$$?; \
	if [ $$ec -eq 0 ] || [ $$ec -eq 1 ]; then exit 0; else exit $$ec; fi

# Finite run for CI / agents: exit after ~3s if the guest idles.
qemu-ci: $(LOADER_ELF)
	$(QEMU) $(QEMU_FLAGS) -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
		-icount shift=1,align=off -rtc clock=vm \
		-watchdog-action none \
		; true

qemu-debug: $(LOADER_ELF)
	$(QEMU) $(QEMU_FLAGS) -s -S

clean:
	rm -rf $(BUILD)
	cd $(KERNEL_DIR) && cargo clean
	cargo clean
