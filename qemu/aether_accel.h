/* SPDX-License-Identifier: MIT OR Apache-2.0
 *
 * Aether path-A virtio-accel BAR — portable device model.
 *
 * Same frozen offsets as docs/ACCEL.md / aether_drivers::mmio::AccelMmio.
 * SoftNPU (I32 Nop/MatMul/Wave) sits behind the doorbell. Tensor DMA uses
 * addresses from AccelJobWire. Host tests (`aether_accel_test.c` and
 * `aether_drivers::path_a`) supply Soft-SMMU IOVA-translating DMA ops so
 * the wire carries non-identity IOVA and a wrong SID aborts. This device
 * is not a QEMU IOMMU; Soft SMMU stays software. Stock `make qemu` is
 * path B (in-kernel BAR).
 *
 * Compiles standalone (host unit test) or as a QEMU softmmu PCI device.
 */

#ifndef AETHER_ACCEL_H
#define AETHER_ACCEL_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define AETHER_ACCEL_BAR_SIZE 0x400u

#define AETHER_ACCEL_MAGIC 0xAE7EACC1u
#define AETHER_ACCEL_VERSION 1u
#define AETHER_ACCEL_QSIZE 8u

#define AETHER_REG_MAGIC 0x00u
#define AETHER_REG_VERSION 0x04u
#define AETHER_REG_STATUS 0x08u
#define AETHER_REG_QSIZE 0x0Cu
#define AETHER_REG_DOORBELL 0x10u
#define AETHER_REG_USED_IDX 0x14u
#define AETHER_REG_IRQ_STATUS 0x18u
#define AETHER_REG_IRQ_ACK 0x1Cu
#define AETHER_REG_AVAIL_IDX 0x20u

#define AETHER_IRQ_USED 1u

#define AETHER_STATUS_ACK 1u
#define AETHER_STATUS_DRIVER 2u
#define AETHER_STATUS_DRIVER_OK 4u
#define AETHER_STATUS_FAILED 128u

#define AETHER_AVAIL_BASE 0x80u
#define AETHER_USED_BASE 0x340u
#define AETHER_JOB_WIRE_SIZE 88u
#define AETHER_USED_WIRE_SIZE 16u

#define AETHER_OP_NOP 0u
#define AETHER_OP_MATMUL 1u
#define AETHER_OP_WAVE 2u

#define AETHER_DTYPE_I32 0u
#define AETHER_DTYPE_F16 1u
#define AETHER_DTYPE_F32 2u

#define AETHER_CPL_OK 0
#define AETHER_CPL_EXEC -1
#define AETHER_CPL_DMA -2

/* PCI ids when built as a QEMU -device (not an upstream virtio id). */
#define AETHER_PCI_VENDOR 0xAE7E
#define AETHER_PCI_DEVICE 0xACC1
#define AETHER_PCI_CLASS 0x1200

typedef struct AetherJobWire {
    uint32_t op;
    uint32_t flags;
    uint32_t m;
    uint32_t n;
    uint32_t k;
    uint32_t tenant;
    uint64_t a;
    uint64_t b;
    uint64_t c;
    uint64_t bias;
    uint64_t fence_id;
    uint32_t a_stride;
    uint32_t b_stride;
    uint32_t c_stride;
    uint32_t completion_ep;
    uint32_t partition;
    uint8_t dtype;
    uint8_t space;
    uint8_t phase;
    uint8_t _pad;
} AetherJobWire;

#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(sizeof(AetherJobWire) == AETHER_JOB_WIRE_SIZE, "AccelJobWire is 88 bytes");
#endif

typedef struct AetherUsedWire {
    uint32_t job_seq;
    int32_t status;
    uint32_t cycles;
    uint32_t _pad;
} AetherUsedWire;

/*
 * Guest DMA callbacks. Return 0 on success, non-zero on fault.
 * Addresses are whatever the job wire published (GPA or Soft-SMMU IOVA).
 * A real IOMMU would sit under pci_dma_*; the host IOVA proof installs
 * that translate in these callbacks. Not a QEMU IOMMU.
 */
typedef int (*AetherDmaRead)(void *ctx, uint64_t gpa, void *buf, size_t len);
typedef int (*AetherDmaWrite)(void *ctx, uint64_t gpa, const void *buf, size_t len);
typedef void (*AetherIrqUpdate)(void *ctx, int level);

typedef struct AetherAccelDev {
    uint8_t bar[AETHER_ACCEL_BAR_SIZE];
    uint32_t seq;
    uint32_t dev_cursor;
    void *dma_ctx;
    AetherDmaRead dma_read;
    AetherDmaWrite dma_write;
    AetherIrqUpdate irq_update;
} AetherAccelDev;

void aether_accel_init(AetherAccelDev *d);
uint32_t aether_accel_read32(AetherAccelDev *d, uint32_t off);
void aether_accel_write32(AetherAccelDev *d, uint32_t off, uint32_t val);

/* Drain the doorbell: take avail jobs, DMA tensors, write used, raise IRQ. */
void aether_accel_service(AetherAccelDev *d);

#ifdef __cplusplus
}
#endif

#endif /* AETHER_ACCEL_H */
