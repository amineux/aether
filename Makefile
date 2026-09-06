# Aether — accelerator-first fabric kernel
# `make qemu` is the demo. `make test` is host-side logic.

TARGET      := x86_64-unknown-none
KERNEL_DIR  := kernel
USER_DIR    := user/init
KERNEL_ELF  := $(KERNEL_DIR)/target/$(TARGET)/release/aether
INIT_ELF    := $(USER_DIR)/target/$(TARGET)/release/aether-init
BUILD       := build
KERNEL_BIN  := $(BUILD)/kernel.bin
LOADER_ELF  := $(BUILD)/aether.elf
INIT_BLOB   := $(BUILD)/init.elf
QEMU        := qemu-system-x86_64
QEMU_FLAGS  := -kernel $(LOADER_ELF) -serial stdio -display none \
               -no-reboot -no-shutdown -m 128M \
               -device isa-debug-exit,iobase=0xf4,iosize=0x04

RV_TARGET   := riscv64gc-unknown-none-elf
RV_KERNEL   := $(KERNEL_DIR)/target/$(RV_TARGET)/release/aether
RV_ELF      := $(BUILD)/aether-riscv.elf
QEMU_RV     := qemu-system-riscv64
QEMU_RV_FLAGS := -machine virt -cpu rv64 -m 128M -nographic \
                 -no-reboot -kernel $(RV_ELF)

.PHONY: all kernel kernel-riscv loader user-init qemu qemu-riscv \
        qemu-debug qemu-ci qemu-riscv-ci test test-host target target-riscv \
        clean help

all: $(LOADER_ELF)

help:
	@echo "Aether targets:"
	@echo "  make test         - host unit tests (caps, fabric, arenas, sched, L, elf)"
	@echo "  make qemu         - x86_64 /init + kernel, boot under QEMU"
	@echo "  make qemu-riscv   - RISC-V virt thin port (kmain + aether_core demo)"
	@echo "  make qemu-ci      - x86_64 finite CI boot"
	@echo "  make qemu-riscv-ci - RISC-V CI boot; greps the fabric banner"
	@echo "  make clean"

target:
	rustup target add $(TARGET)

target-riscv:
	rustup target add $(RV_TARGET)

test: test-host

test-host:
	cargo test --workspace

user-init: target $(INIT_BLOB)

$(INIT_BLOB): $(USER_DIR)/src/main.rs $(USER_DIR)/user.ld $(USER_DIR)/Cargo.toml
	mkdir -p $(BUILD)
	cd $(USER_DIR) && cargo build --release --target $(TARGET)
	cp $(INIT_ELF) $(INIT_BLOB)
	@echo "init.elf $$(wc -c < $(INIT_BLOB)) bytes (static non-PIE ELF64)"

kernel: target $(INIT_BLOB)
	cd $(KERNEL_DIR) && cargo build --release --target $(TARGET)
	@touch $(KERNEL_ELF)

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

# CI: same success semantics, but fail the job if the guest hangs.
qemu-ci: $(LOADER_ELF)
	timeout 45s $(QEMU) $(QEMU_FLAGS); \
	ec=$$?; \
	if [ $$ec -eq 0 ] || [ $$ec -eq 1 ]; then exit 0; else exit $$ec; fi

qemu-debug: $(LOADER_ELF)
	$(QEMU) $(QEMU_FLAGS) -s -S

kernel-riscv: target-riscv
	cd $(KERNEL_DIR) && cargo build --release --target $(RV_TARGET)
	mkdir -p $(BUILD)
	cp -f $(RV_KERNEL) $(RV_ELF)
	@echo "riscv kernel $$(wc -c < $(RV_ELF)) bytes"

$(RV_ELF): kernel-riscv

# sifive_test at 0x100000: write 0x5555 → qemu exit 0.
qemu-riscv: $(RV_ELF)
	$(QEMU_RV) $(QEMU_RV_FLAGS); \
	ec=$$?; \
	if [ $$ec -eq 0 ] || [ $$ec -eq 1 ]; then exit 0; else exit $$ec; fi

qemu-riscv-ci: $(RV_ELF)
	mkdir -p $(BUILD)
	rm -f $(BUILD)/riscv-serial.log
	set +e; \
	timeout --signal=KILL 25s $(QEMU_RV) $(QEMU_RV_FLAGS) \
		> $(BUILD)/riscv-serial.log 2>&1; \
	ec=$$?; \
	set -e; \
	cat $(BUILD)/riscv-serial.log; \
	if grep -q "FABRIC IPC + TENSOR ARENA + ACCEL JOB COMPLETE" $(BUILD)/riscv-serial.log; then \
		echo "qemu-riscv-ci: demo ok (qemu exit $$ec)"; \
		exit 0; \
	fi; \
	echo "qemu-riscv-ci: demo banner missing (qemu exit $$ec)"; \
	exit 1

clean:
	rm -rf $(BUILD)
	cd $(KERNEL_DIR) && cargo clean
	cd $(USER_DIR) && cargo clean
	cargo clean
