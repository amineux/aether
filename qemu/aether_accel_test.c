/* SPDX-License-Identifier: MIT OR Apache-2.0
 *
 * Host unit test of the path-A device model. No QEMU required.
 *
 * Covers the frozen BAR, doorbell → SoftNPU I32 DMA, used-ring IRQ,
 * and a Soft-SMMU IOVA / wrong-SID proof on this path-A BAR (DMA
 * callbacks translate; the device is not a QEMU IOMMU).
 * This is what CI runs. A patched QEMU binary is optional (QEMU_ACCEL).
 * Stock `make qemu` stays path B.
 */

#include "aether_accel.h"

#include <stdio.h>
#include <string.h>

#define GUEST_RAM 256

struct GuestRam {
    uint8_t bytes[GUEST_RAM];
};

static int g_irq;

static int ram_read(void *ctx, uint64_t gpa, void *buf, size_t len)
{
    struct GuestRam *ram = ctx;
    if (gpa + len > GUEST_RAM) {
        return -1;
    }
    memcpy(buf, ram->bytes + (size_t)gpa, len);
    return 0;
}

static int ram_write(void *ctx, uint64_t gpa, const void *buf, size_t len)
{
    struct GuestRam *ram = ctx;
    if (gpa + len > GUEST_RAM) {
        return -1;
    }
    memcpy(ram->bytes + (size_t)gpa, buf, len);
    return 0;
}

static void irq_update(void *ctx, int level)
{
    (void)ctx;
    g_irq = level;
}

static int fail(const char *msg)
{
    fprintf(stderr, "FAIL: %s\n", msg);
    return 1;
}

static void poke_job(AetherAccelDev *d, uint32_t slot, const AetherJobWire *job)
{
    uint32_t off = AETHER_AVAIL_BASE + slot * AETHER_JOB_WIRE_SIZE;
    memcpy(d->bar + off, job, AETHER_JOB_WIRE_SIZE);
}

static AetherUsedWire peek_used(const AetherAccelDev *d, uint32_t slot)
{
    AetherUsedWire u;
    uint32_t off = AETHER_USED_BASE + slot * AETHER_USED_WIRE_SIZE;
    memcpy(&u, d->bar + off, AETHER_USED_WIRE_SIZE);
    return u;
}

static AetherJobWire matmul_2x2(void)
{
    AetherJobWire j;
    memset(&j, 0, sizeof(j));
    j.op = AETHER_OP_MATMUL;
    j.m = 2;
    j.n = 2;
    j.k = 2;
    j.a = 0;
    j.b = 16;
    j.c = 32;
    j.a_stride = 2;
    j.b_stride = 2;
    j.c_stride = 2;
    j.dtype = AETHER_DTYPE_I32;
    j.tenant = 1;
    return j;
}

static void fill_2x2(uint8_t *bytes)
{
    /* A = [1 2; 3 4], B = [5 6; 7 8] → C = [19 22; 43 50] */
    int32_t a[4] = {1, 2, 3, 4};
    int32_t b[4] = {5, 6, 7, 8};
    memcpy(bytes + 0, a, sizeof(a));
    memcpy(bytes + 16, b, sizeof(b));
}

static int test_frozen_offsets(void)
{
    if (AETHER_REG_MAGIC != 0x00 || AETHER_REG_VERSION != 0x04
        || AETHER_REG_STATUS != 0x08 || AETHER_REG_QSIZE != 0x0C
        || AETHER_REG_DOORBELL != 0x10 || AETHER_REG_USED_IDX != 0x14
        || AETHER_REG_IRQ_STATUS != 0x18 || AETHER_REG_IRQ_ACK != 0x1C
        || AETHER_REG_AVAIL_IDX != 0x20 || AETHER_AVAIL_BASE != 0x80
        || AETHER_USED_BASE != 0x340 || AETHER_JOB_WIRE_SIZE != 88
        || AETHER_USED_WIRE_SIZE != 16 || AETHER_ACCEL_QSIZE != 8
        || AETHER_ACCEL_MAGIC != 0xAE7EACC1u || AETHER_ACCEL_VERSION != 1) {
        return fail("frozen BAR offsets / identity");
    }
    if (sizeof(AetherJobWire) != 88) {
        return fail("AccelJobWire size");
    }
    return 0;
}

static int test_reset_values(void)
{
    AetherAccelDev d;
    memset(&d, 0, sizeof(d));
    aether_accel_init(&d);
    if (aether_accel_read32(&d, AETHER_REG_MAGIC) != AETHER_ACCEL_MAGIC) {
        return fail("reset magic");
    }
    if (aether_accel_read32(&d, AETHER_REG_VERSION) != AETHER_ACCEL_VERSION) {
        return fail("reset version");
    }
    if (aether_accel_read32(&d, AETHER_REG_STATUS) != 0) {
        return fail("reset status");
    }
    if (aether_accel_read32(&d, AETHER_REG_QSIZE) != AETHER_ACCEL_QSIZE) {
        return fail("reset qsize");
    }
    if (aether_accel_read32(&d, AETHER_REG_DOORBELL) != 0) {
        return fail("reset doorbell");
    }
    if (aether_accel_read32(&d, AETHER_REG_USED_IDX) != 0) {
        return fail("reset used_idx");
    }
    /* Guest cannot rewrite magic / version / qsize. */
    aether_accel_write32(&d, AETHER_REG_MAGIC, 0);
    aether_accel_write32(&d, AETHER_REG_VERSION, 99);
    aether_accel_write32(&d, AETHER_REG_QSIZE, 3);
    if (aether_accel_read32(&d, AETHER_REG_MAGIC) != AETHER_ACCEL_MAGIC
        || aether_accel_read32(&d, AETHER_REG_VERSION) != AETHER_ACCEL_VERSION
        || aether_accel_read32(&d, AETHER_REG_QSIZE) != AETHER_ACCEL_QSIZE) {
        return fail("ROM cfg overwritten");
    }
    return 0;
}

static void negotiate(AetherAccelDev *d)
{
    aether_accel_write32(d, AETHER_REG_STATUS,
                         AETHER_STATUS_ACK | AETHER_STATUS_DRIVER | AETHER_STATUS_DRIVER_OK);
}

static int test_matmul_doorbell_used_irq(void)
{
    AetherAccelDev d;
    struct GuestRam ram;
    AetherJobWire job;
    AetherUsedWire used;
    int32_t c00, c01, c10, c11;

    memset(&d, 0, sizeof(d));
    memset(&ram, 0, sizeof(ram));
    g_irq = 0;
    fill_2x2(ram.bytes);
    d.dma_ctx = &ram;
    d.dma_read = ram_read;
    d.dma_write = ram_write;
    d.irq_update = irq_update;
    aether_accel_init(&d);
    negotiate(&d);

    job = matmul_2x2();
    poke_job(&d, 0, &job);
    aether_accel_write32(&d, AETHER_REG_AVAIL_IDX, 1);
    if (aether_accel_read32(&d, AETHER_REG_USED_IDX) != 0) {
        return fail("used_idx before kick");
    }
    aether_accel_write32(&d, AETHER_REG_DOORBELL, 1);

    if (aether_accel_read32(&d, AETHER_REG_DOORBELL) != 0) {
        return fail("doorbell should clear after service");
    }
    if (aether_accel_read32(&d, AETHER_REG_USED_IDX) != 1) {
        return fail("used_idx after complete");
    }
    if ((aether_accel_read32(&d, AETHER_REG_IRQ_STATUS) & AETHER_IRQ_USED) == 0) {
        return fail("irq_status used bit");
    }
    if (!g_irq) {
        return fail("IRQ line");
    }

    used = peek_used(&d, 0);
    if (used.job_seq != 1 || used.status != 0 || used.cycles != 8) {
        fprintf(stderr, "used seq=%u status=%d cycles=%u\n", used.job_seq, used.status,
                used.cycles);
        return fail("used-ring completion");
    }

    memcpy(&c00, ram.bytes + 32, 4);
    memcpy(&c01, ram.bytes + 36, 4);
    memcpy(&c10, ram.bytes + 40, 4);
    memcpy(&c11, ram.bytes + 44, 4);
    if (c00 != 19 || c01 != 22 || c10 != 43 || c11 != 50) {
        fprintf(stderr, "C = [%d %d; %d %d]\n", c00, c01, c10, c11);
        return fail("SoftNPU I32 matmul DMA");
    }

    aether_accel_write32(&d, AETHER_REG_IRQ_ACK, 1);
    if ((aether_accel_read32(&d, AETHER_REG_IRQ_STATUS) & AETHER_IRQ_USED) != 0) {
        return fail("irq ack");
    }
    if (g_irq) {
        return fail("IRQ line after ack");
    }

    /* Published cfg after one submit/complete (path-B golden values). */
    if (aether_accel_read32(&d, AETHER_REG_MAGIC) != AETHER_ACCEL_MAGIC
        || aether_accel_read32(&d, AETHER_REG_VERSION) != AETHER_ACCEL_VERSION
        || aether_accel_read32(&d, AETHER_REG_STATUS)
               != (AETHER_STATUS_ACK | AETHER_STATUS_DRIVER | AETHER_STATUS_DRIVER_OK)
        || aether_accel_read32(&d, AETHER_REG_QSIZE) != AETHER_ACCEL_QSIZE
        || aether_accel_read32(&d, AETHER_REG_DOORBELL) != 0
        || aether_accel_read32(&d, AETHER_REG_USED_IDX) != 1) {
        return fail("published cfg after complete");
    }
    return 0;
}

static int test_nop_and_dma_fault(void)
{
    AetherAccelDev d;
    struct GuestRam ram;
    AetherJobWire job;
    AetherUsedWire used;

    memset(&d, 0, sizeof(d));
    memset(&ram, 0, sizeof(ram));
    d.dma_ctx = &ram;
    d.dma_read = ram_read;
    d.dma_write = ram_write;
    aether_accel_init(&d);
    negotiate(&d);

    memset(&job, 0, sizeof(job));
    job.op = AETHER_OP_NOP;
    poke_job(&d, 0, &job);
    aether_accel_write32(&d, AETHER_REG_AVAIL_IDX, 1);
    aether_accel_write32(&d, AETHER_REG_DOORBELL, 1);
    used = peek_used(&d, 0);
    if (used.status != 0 || used.cycles != 1) {
        return fail("nop completion");
    }

    job = matmul_2x2();
    job.a = 0x1000; /* outside guest RAM */
    poke_job(&d, 1, &job);
    aether_accel_write32(&d, AETHER_REG_AVAIL_IDX, 2);
    aether_accel_write32(&d, AETHER_REG_DOORBELL, 1);
    used = peek_used(&d, 1);
    if (used.status != AETHER_CPL_DMA) {
        fprintf(stderr, "dma fault status=%d\n", used.status);
        return fail("DMA fault completion");
    }
    return 0;
}

static int test_f32_refused(void)
{
    AetherAccelDev d;
    struct GuestRam ram;
    AetherJobWire job;
    AetherUsedWire used;

    memset(&d, 0, sizeof(d));
    memset(&ram, 0, sizeof(ram));
    d.dma_ctx = &ram;
    d.dma_read = ram_read;
    d.dma_write = ram_write;
    aether_accel_init(&d);
    job = matmul_2x2();
    job.dtype = AETHER_DTYPE_F32;
    poke_job(&d, 0, &job);
    aether_accel_write32(&d, AETHER_REG_AVAIL_IDX, 1);
    aether_accel_write32(&d, AETHER_REG_DOORBELL, 1);
    used = peek_used(&d, 0);
    if (used.status != AETHER_CPL_EXEC) {
        return fail("F32 should stay path-B SoftNPU");
    }
    return 0;
}

/*
 * Path-A Soft-SMMU intercept under DMA. Matches aether_core::iommu
 * SOFT_SMMU_IOVA_BASE and drivers::path_a::PATH_A_SSID. Not a QEMU IOMMU.
 */
#define SOFT_SMMU_IOVA_BASE 0x000100000000ULL
#define PATH_A_SID 4u
#define PATH_B_SOFTNPU_SID 0u

struct SmmuRam {
    uint8_t bytes[GUEST_RAM];
    uint32_t dma_sid;
    uint32_t bound_sid;
    int bound;
    uint64_t iova;
    uint64_t gpa;
    uint64_t len;
};

static int smmu_translate(const struct SmmuRam *s, uint64_t addr, uint64_t *gpa_out)
{
    uint64_t off;
    if (!s->bound) {
        return -1;
    }
    if (s->dma_sid != s->bound_sid) {
        return -1; /* wrong SID */
    }
    if (addr < SOFT_SMMU_IOVA_BASE) {
        return -1; /* identity GPA refused once Soft SMMU is bound */
    }
    if (addr < s->iova || addr >= s->iova + s->len) {
        return -1;
    }
    off = addr - s->iova;
    *gpa_out = s->gpa + off;
    return 0;
}

static int smmu_read(void *ctx, uint64_t addr, void *buf, size_t len)
{
    struct SmmuRam *s = ctx;
    uint64_t gpa;
    if (smmu_translate(s, addr, &gpa) != 0) {
        return -1;
    }
    if (gpa + len > GUEST_RAM) {
        return -1;
    }
    memcpy(buf, s->bytes + (size_t)gpa, len);
    return 0;
}

static int smmu_write(void *ctx, uint64_t addr, const void *buf, size_t len)
{
    struct SmmuRam *s = ctx;
    uint64_t gpa;
    if (smmu_translate(s, addr, &gpa) != 0) {
        return -1;
    }
    if (gpa + len > GUEST_RAM) {
        return -1;
    }
    memcpy(s->bytes + (size_t)gpa, buf, len);
    return 0;
}

static AetherJobWire matmul_2x2_iova(uint64_t iova)
{
    AetherJobWire j = matmul_2x2();
    j.a = iova;
    j.b = iova + 16;
    j.c = iova + 32;
    j.flags = PATH_A_SID;
    return j;
}

static void smmu_bind_path_a(struct SmmuRam *s)
{
    memset(s, 0, sizeof(*s));
    fill_2x2(s->bytes);
    s->bound = 1;
    s->bound_sid = PATH_A_SID;
    s->dma_sid = PATH_A_SID;
    s->iova = SOFT_SMMU_IOVA_BASE;
    s->gpa = 0;
    s->len = GUEST_RAM;
}

static int test_iova_not_identity_and_wrong_sid(void)
{
    AetherAccelDev d;
    struct SmmuRam ram;
    AetherJobWire job;
    AetherUsedWire used;
    int32_t c00, c01, c10, c11;
    uint32_t off;

    smmu_bind_path_a(&ram);
    memset(&d, 0, sizeof(d));
    d.dma_ctx = &ram;
    d.dma_read = smmu_read;
    d.dma_write = smmu_write;
    aether_accel_init(&d);
    negotiate(&d);

    /* Why path A: this is the portable BAR device DMA-reading AccelJobWire,
     * not SoftNpuDevice walking the in-kernel array on DEFAULT_STREAM. */
    if (ram.bound_sid == PATH_B_SOFTNPU_SID) {
        return fail("path-A SID must not be SoftNPU ssid 0");
    }
    if (ram.iova == ram.gpa || ram.iova < SOFT_SMMU_IOVA_BASE) {
        return fail("path-A IOVA must not be identity");
    }

    job = matmul_2x2_iova(ram.iova);
    poke_job(&d, 0, &job);
    off = AETHER_AVAIL_BASE;
    {
        AetherJobWire wire;
        memcpy(&wire, d.bar + off, AETHER_JOB_WIRE_SIZE);
        if (wire.a != ram.iova || wire.b != ram.iova + 16 || wire.c != ram.iova + 32) {
            return fail("path-A job wire must carry IOVA, not GPA");
        }
        if (wire.flags != PATH_A_SID) {
            return fail("path-A job wire flags must stamp stream_id");
        }
        if (wire.a == 0 || wire.b == 16 || wire.c == 32) {
            return fail("path-A DMA addresses were identity GPAs");
        }
    }
    aether_accel_write32(&d, AETHER_REG_AVAIL_IDX, 1);
    aether_accel_write32(&d, AETHER_REG_DOORBELL, 1);
    used = peek_used(&d, 0);
    if (used.status != 0) {
        fprintf(stderr, "iova matmul status=%d\n", used.status);
        return fail("path-A IOVA matmul");
    }
    memcpy(&c00, ram.bytes + 32, 4);
    memcpy(&c01, ram.bytes + 36, 4);
    memcpy(&c10, ram.bytes + 40, 4);
    memcpy(&c11, ram.bytes + 44, 4);
    if (c00 != 19 || c01 != 22 || c10 != 43 || c11 != 50) {
        fprintf(stderr, "C = [%d %d; %d %d]\n", c00, c01, c10, c11);
        return fail("path-A Soft-SMMU IOVA DMA matmul");
    }

    /* Identity GPA in the wire must abort once Soft SMMU is bound. */
    memset(&ram.bytes[32], 0, 16);
    job = matmul_2x2(); /* a=0, b=16, c=32 guest PA */
    poke_job(&d, 1, &job);
    aether_accel_write32(&d, AETHER_REG_AVAIL_IDX, 2);
    aether_accel_write32(&d, AETHER_REG_DOORBELL, 1);
    used = peek_used(&d, 1);
    if (used.status != AETHER_CPL_DMA) {
        fprintf(stderr, "identity GPA status=%d\n", used.status);
        return fail("identity GPA must abort on path-A Soft SMMU");
    }

    /* Wrong SID: DMA walks path-B SoftNPU ssid 0 against path-A pins. */
    ram.dma_sid = PATH_B_SOFTNPU_SID;
    job = matmul_2x2_iova(ram.iova);
    poke_job(&d, 2, &job);
    aether_accel_write32(&d, AETHER_REG_AVAIL_IDX, 3);
    aether_accel_write32(&d, AETHER_REG_DOORBELL, 1);
    used = peek_used(&d, 2);
    if (used.status != AETHER_CPL_DMA) {
        fprintf(stderr, "wrong SID status=%d\n", used.status);
        return fail("wrong-SID path-A DMA must abort");
    }
    memcpy(&c00, ram.bytes + 32, 4);
    if (c00 != 0) {
        return fail("wrong SID must not write C");
    }
    return 0;
}

static int test_wave_bias(void)
{
    AetherAccelDev d;
    struct GuestRam ram;
    AetherJobWire job;
    int32_t c00;
    int32_t bias[2] = {1, 2};

    memset(&d, 0, sizeof(d));
    memset(&ram, 0, sizeof(ram));
    fill_2x2(ram.bytes);
    memcpy(ram.bytes + 64, bias, sizeof(bias));
    d.dma_ctx = &ram;
    d.dma_read = ram_read;
    d.dma_write = ram_write;
    aether_accel_init(&d);
    job = matmul_2x2();
    job.op = AETHER_OP_WAVE;
    job.bias = 64;
    poke_job(&d, 0, &job);
    aether_accel_write32(&d, AETHER_REG_AVAIL_IDX, 1);
    aether_accel_write32(&d, AETHER_REG_DOORBELL, 1);
    memcpy(&c00, ram.bytes + 32, 4);
    if (c00 != 20) { /* 19 + bias[0] */
        fprintf(stderr, "wave c00=%d\n", c00);
        return fail("wave + bias");
    }
    return 0;
}

int main(void)
{
    int rc = 0;
    rc |= test_frozen_offsets();
    rc |= test_reset_values();
    rc |= test_matmul_doorbell_used_irq();
    rc |= test_nop_and_dma_fault();
    rc |= test_f32_refused();
    rc |= test_wave_bias();
    rc |= test_iova_not_identity_and_wrong_sid();
    if (rc == 0) {
        printf("aether-accel: path-A device model ok "
               "(BAR + SoftNPU I32 DMA + used-ring IRQ; "
               "Soft-SMMU IOVA + wrong-SID abort)\n");
    }
    return rc;
}
