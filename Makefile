# Aether — accelerator-first fabric kernel
# `make qemu` is the demo. `make test` is host-side logic.

TARGET      := x86_64-unknown-none
KERNEL_DIR  := kernel
USER_DIR    := user/init
PROBE_DIR   := user/probe
KERNEL_ELF  := $(KERNEL_DIR)/target/$(TARGET)/release/aether
INIT_ELF    := $(USER_DIR)/target/$(TARGET)/release/aether-init
PROBE_ELF   := $(PROBE_DIR)/target/$(TARGET)/release/aether-probe
BUILD       := build
KERNEL_BIN  := $(BUILD)/kernel.bin
LOADER_ELF  := $(BUILD)/aether.elf
INIT_BLOB   := $(BUILD)/init.elf
PROBE_BLOB  := $(BUILD)/probe.elf
QEMU        := qemu-system-x86_64
QEMU_FLAGS  := -kernel $(LOADER_ELF) -serial stdio -display none \
               -no-reboot -no-shutdown -m 128M \
               -cpu qemu64,+smep,+smap \
               -device isa-debug-exit,iobase=0xf4,iosize=0x04

RV_TARGET   := riscv64gc-unknown-none-elf
RV_KERNEL   := $(KERNEL_DIR)/target/$(RV_TARGET)/release/aether
RV_ELF      := $(BUILD)/aether-riscv.elf
RV_INIT_ELF := $(USER_DIR)/target/$(RV_TARGET)/release/aether-init
RV_INIT_BLOB := $(BUILD)/init-riscv.elf
QEMU_RV     := qemu-system-riscv64
QEMU_RV_FLAGS := -machine virt -cpu rv64 -m 128M -nographic \
                 -no-reboot -kernel $(RV_ELF)

AA_TARGET   := aarch64-unknown-none
AA_KERNEL   := $(KERNEL_DIR)/target/$(AA_TARGET)/release/aether
AA_ELF      := $(BUILD)/aether-aarch64.elf
QEMU_AA     := qemu-system-aarch64
# QEMU virt, GICv2 + cortex-a72, PL011 UART on stdio. Semihosting is
# the clean exit path (Angel SYS_EXIT); CI also greps the fabric banner.
QEMU_AA_FLAGS := -machine virt,gic-version=2 -cpu cortex-a72 -m 128M \
                 -nographic -no-reboot -nic none -kernel $(AA_ELF) \
                 -semihosting

.PHONY: all kernel kernel-riscv kernel-aarch64 loader user-init user-init-riscv \
        user-probe \
        qemu qemu-riscv qemu-aarch64 \
        qemu-debug qemu-ci qemu-riscv-ci qemu-aarch64-ci qemu-smp qemu-smp-ci \
        test test-host target target-riscv target-aarch64 clean help

all: $(LOADER_ELF)

help:
	@echo "Aether targets:"
	@echo "  make test         - host unit tests (caps, fabric, arenas, sched, L, elf)"
	@echo "  make qemu         - x86_64 /init + kernel, boot under QEMU"
	@echo "  make qemu-riscv   - RISC-V virt S-mode + U-mode /init (no PLIC)"
	@echo "  make qemu-aarch64 - aarch64 virt thin port (kmain + aether_core demo)"
	@echo "  make qemu-ci      - x86_64 finite CI boot (mmap + HH + SMEP/SMAP + aspace greps)"
	@echo "  make qemu-smp     - x86_64 boot with -smp 2 (INIT-SIPI smoke)"
	@echo "  make qemu-smp-ci  - SMP smoke; greps AP online + work-steal + fabric"
	@echo "  make qemu-riscv-ci - RISC-V CI boot; greps U-mode /init + fabric"
	@echo "  make qemu-aarch64-ci - aarch64 CI boot; greps the fabric banner"
	@echo "  make clean"

target:
	rustup target add $(TARGET)

target-riscv:
	rustup target add $(RV_TARGET)

target-aarch64:
	rustup target add $(AA_TARGET)

test: test-host

test-host:
	cargo test --workspace

user-init: target $(INIT_BLOB)
user-probe: target $(PROBE_BLOB)

$(INIT_BLOB): $(USER_DIR)/src/main.rs $(USER_DIR)/user.ld $(USER_DIR)/Cargo.toml
	mkdir -p $(BUILD)
	cd $(USER_DIR) && cargo build --release --target $(TARGET)
	cp $(INIT_ELF) $(INIT_BLOB)
	@echo "init.elf $$(wc -c < $(INIT_BLOB)) bytes (static non-PIE ELF64)"

$(PROBE_BLOB): $(PROBE_DIR)/src/main.rs $(PROBE_DIR)/user.ld $(PROBE_DIR)/Cargo.toml
	mkdir -p $(BUILD)
	cd $(PROBE_DIR) && cargo build --release --target $(TARGET)
	cp $(PROBE_ELF) $(PROBE_BLOB)
	@echo "probe.elf $$(wc -c < $(PROBE_BLOB)) bytes (static non-PIE ELF64)"

kernel: target $(INIT_BLOB) $(PROBE_BLOB)
	cd $(KERNEL_DIR) && cargo build --release --target $(TARGET)
	@touch $(KERNEL_ELF)

$(KERNEL_ELF): kernel

$(KERNEL_BIN): $(KERNEL_ELF)
	mkdir -p $(BUILD)
	objcopy -O binary $(KERNEL_ELF) $(KERNEL_BIN)
	@sz=$$(wc -c < $(KERNEL_BIN)); \
	if [ $$sz -gt 16777216 ]; then \
		echo "kernel.bin $$sz bytes — HH VMA gap? objcopy produced a huge image"; \
		exit 1; \
	fi
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
	mkdir -p $(BUILD)
	rm -f $(BUILD)/qemu-serial.log
	set +e; \
	timeout --signal=KILL 45s $(QEMU) $(QEMU_FLAGS) \
		> $(BUILD)/qemu-serial.log 2>&1; \
	ec=$$?; \
	set -e; \
	cat $(BUILD)/qemu-serial.log; \
	if { [ $$ec -eq 0 ] || [ $$ec -eq 1 ]; } \
	   && grep -q "\\[mm\\] mmap: multiboot1" $(BUILD)/qemu-serial.log \
	   && grep -q "\\[mm\\] frames mmap clip=16MiB cap=128MiB" $(BUILD)/qemu-serial.log \
	   && grep -q "\\[mm\\] SMEP+SMAP" $(BUILD)/qemu-serial.log \
	   && grep -q "\\[mm\\] higher-half ok" $(BUILD)/qemu-serial.log \
	   && grep -q "\\[mm\\] aspace isolate ok" $(BUILD)/qemu-serial.log \
	   && grep -q "\\[cdt\\] revoke descendants ok" $(BUILD)/qemu-serial.log \
	   && grep -q "\\[sparsify\\] below-threshold DROP" $(BUILD)/qemu-serial.log \
	   && grep -q "\\[fence\\] timeline seq#" $(BUILD)/qemu-serial.log \
	   && grep -q "\\[accel\\] SoftNPU F32/F16 soft-float" $(BUILD)/qemu-serial.log \
	   && grep -q "\\[probe\\] ring-3 /probe" $(BUILD)/qemu-serial.log \
	   && grep -q "FABRIC IPC + TENSOR ARENA + ACCEL JOB COMPLETE" $(BUILD)/qemu-serial.log; then \
		echo "qemu-ci: /init + mmap + HH + SMEP/SMAP + per-task PML4 + CDT ok (qemu exit $$ec)"; \
		exit 0; \
	fi; \
	echo "qemu-ci: demo/aspace banner missing or bad exit (qemu exit $$ec)"; \
	exit 1

qemu-debug: $(LOADER_ELF)
	$(QEMU) $(QEMU_FLAGS) -s -S

# SMP smoke: same guest as qemu-ci plus -smp 2. UP qemu-ci is unchanged.
QEMU_SMP_FLAGS := $(QEMU_FLAGS) -smp 2

qemu-smp: $(LOADER_ELF)
	$(QEMU) $(QEMU_SMP_FLAGS); \
	ec=$$?; \
	if [ $$ec -eq 0 ] || [ $$ec -eq 1 ]; then exit 0; else exit $$ec; fi

qemu-smp-ci: $(LOADER_ELF)
	mkdir -p $(BUILD)
	rm -f $(BUILD)/smp-serial.log
	set +e; \
	timeout --signal=KILL 45s $(QEMU) $(QEMU_SMP_FLAGS) \
		> $(BUILD)/smp-serial.log 2>&1; \
	ec=$$?; \
	set -e; \
	cat $(BUILD)/smp-serial.log; \
	if grep -q "\\[smp\\] AP 1 online" $(BUILD)/smp-serial.log \
	   && grep -q "\\[smp\\] SMP smoke ok" $(BUILD)/smp-serial.log \
	   && grep -q "\\[mm\\] mmap: multiboot1" $(BUILD)/smp-serial.log \
	   && grep -q "\\[mm\\] higher-half ok" $(BUILD)/smp-serial.log \
	   && grep -q "\\[mm\\] aspace isolate ok" $(BUILD)/smp-serial.log \
	   && grep -q "\\[cdt\\] revoke descendants ok" $(BUILD)/smp-serial.log \
	   && grep -q "\\[sparsify\\] below-threshold DROP" $(BUILD)/smp-serial.log \
	   && grep -q "\\[fence\\] timeline seq#" $(BUILD)/smp-serial.log \
	   && grep -q "\\[accel\\] SoftNPU F32/F16 soft-float" $(BUILD)/smp-serial.log \
	   && grep -q "FABRIC IPC + TENSOR ARENA + ACCEL JOB COMPLETE" $(BUILD)/smp-serial.log; then \
		echo "qemu-smp-ci: SMP + SoftNPU demo ok (qemu exit $$ec)"; \
		exit 0; \
	fi; \
	echo "qemu-smp-ci: SMP/demo banner missing (qemu exit $$ec)"; \
	exit 1

user-init-riscv: target-riscv $(RV_INIT_BLOB)

$(RV_INIT_BLOB): $(USER_DIR)/src/main.rs $(USER_DIR)/user-riscv.ld $(USER_DIR)/Cargo.toml
	mkdir -p $(BUILD)
	cd $(USER_DIR) && cargo build --release --target $(RV_TARGET)
	cp $(RV_INIT_ELF) $(RV_INIT_BLOB)
	@echo "init-riscv.elf $$(wc -c < $(RV_INIT_BLOB)) bytes (static non-PIE ELF64)"

kernel-riscv: target-riscv $(RV_INIT_BLOB)
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
	timeout --signal=KILL 45s $(QEMU_RV) $(QEMU_RV_FLAGS) \
		> $(BUILD)/riscv-serial.log 2>&1; \
	ec=$$?; \
	set -e; \
	cat $(BUILD)/riscv-serial.log; \
	if grep -q "FABRIC IPC + TENSOR ARENA + ACCEL JOB COMPLETE" $(BUILD)/riscv-serial.log \
	   && grep -q "\\[mm\\] mmap: fallback" $(BUILD)/riscv-serial.log \
	   && grep -q "\\[cdt\\] revoke descendants ok" $(BUILD)/riscv-serial.log \
	   && grep -q "\\[sparsify\\] below-threshold DROP" $(BUILD)/riscv-serial.log \
	   && grep -q "\\[fence\\] timeline seq#" $(BUILD)/riscv-serial.log \
	   && grep -q "\\[accel\\] SoftNPU F32/F16 soft-float" $(BUILD)/riscv-serial.log \
	   && grep -q "\\[mm\\] aspace isolate ok" $(BUILD)/riscv-serial.log \
	   && grep -q "\\[init\\] U-mode /init" $(BUILD)/riscv-serial.log \
	   && grep -q "ecall debug_print ok" $(BUILD)/riscv-serial.log \
	   && grep -q "U-MODE /init VIA ECALL/SRET" $(BUILD)/riscv-serial.log; then \
		echo "qemu-riscv-ci: U-mode /init + demo ok (qemu exit $$ec)"; \
		exit 0; \
	fi; \
	echo "qemu-riscv-ci: userspace/demo banner missing (qemu exit $$ec)"; \
	exit 1

kernel-aarch64: target-aarch64
	cd $(KERNEL_DIR) && cargo build --release --target $(AA_TARGET)
	mkdir -p $(BUILD)
	cp -f $(AA_KERNEL) $(AA_ELF)
	@echo "aarch64 kernel $$(wc -c < $(AA_ELF)) bytes"

$(AA_ELF): kernel-aarch64

# Angel SYS_EXIT 0 via -semihosting; CI greps the same banners as RISC-V.
qemu-aarch64: $(AA_ELF)
	$(QEMU_AA) $(QEMU_AA_FLAGS); \
	ec=$$?; \
	if [ $$ec -eq 0 ] || [ $$ec -eq 1 ]; then exit 0; else exit $$ec; fi

qemu-aarch64-ci: $(AA_ELF)
	mkdir -p $(BUILD)
	rm -f $(BUILD)/aarch64-serial.log
	set +e; \
	timeout --signal=KILL 25s $(QEMU_AA) $(QEMU_AA_FLAGS) \
		> $(BUILD)/aarch64-serial.log 2>&1; \
	ec=$$?; \
	set -e; \
	cat $(BUILD)/aarch64-serial.log; \
	if grep -q "FABRIC IPC + TENSOR ARENA + ACCEL JOB COMPLETE" $(BUILD)/aarch64-serial.log \
	   && grep -q "\\[mm\\] mmap: fallback" $(BUILD)/aarch64-serial.log \
	   && grep -q "\\[cdt\\] revoke descendants ok" $(BUILD)/aarch64-serial.log \
	   && grep -q "\\[sparsify\\] below-threshold DROP" $(BUILD)/aarch64-serial.log \
	   && grep -q "\\[fence\\] timeline seq#" $(BUILD)/aarch64-serial.log \
	   && grep -q "\\[accel\\] SoftNPU F32/F16 soft-float" $(BUILD)/aarch64-serial.log \
	   && grep -q "\\[map\\] Soft SMMU pin + Memory-cap refuse" $(BUILD)/aarch64-serial.log; then \
		echo "qemu-aarch64-ci: demo ok (qemu exit $$ec)"; \
		exit 0; \
	fi; \
	echo "qemu-aarch64-ci: demo banner missing (qemu exit $$ec)"; \
	exit 1

clean:
	rm -rf $(BUILD)
	cd $(KERNEL_DIR) && cargo clean
	cd $(USER_DIR) && cargo clean
	cd $(PROBE_DIR) && cargo clean
	cargo clean
