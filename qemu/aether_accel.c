/* SPDX-License-Identifier: MIT OR Apache-2.0
 *
 * Portable path-A device: frozen BAR + SoftNPU I32 behind the doorbell.
 *
 * When compiled inside a QEMU tree the first include must be qemu/osdep.h.
 */

#if defined(__has_include)
#if __has_include("qemu/osdep.h")
#include "qemu/osdep.h"
#endif
#endif

#include "aether_accel.h"

#include <string.h>

#ifndef INT32_MIN
#include <limits.h>
#endif

static uint32_t load32(const AetherAccelDev *d, uint32_t off)
{
    uint32_t v;
    if ((uint64_t)off + 4u > AETHER_ACCEL_BAR_SIZE) {
        return 0;
    }
    memcpy(&v, d->bar + off, 4);
    return v;
}

static void store32(AetherAccelDev *d, uint32_t off, uint32_t v)
{
    if ((uint64_t)off + 4u > AETHER_ACCEL_BAR_SIZE) {
        return;
    }
    memcpy(d->bar + off, &v, 4);
}

static void raise_irq(AetherAccelDev *d)
{
    uint32_t irq = load32(d, AETHER_REG_IRQ_STATUS) | AETHER_IRQ_USED;
    store32(d, AETHER_REG_IRQ_STATUS, irq);
    if (d->irq_update) {
        d->irq_update(d->dma_ctx, 1);
    }
}

static void lower_irq(AetherAccelDev *d)
{
    uint32_t irq = load32(d, AETHER_REG_IRQ_STATUS) & ~AETHER_IRQ_USED;
    store32(d, AETHER_REG_IRQ_STATUS, irq);
    if (d->irq_update) {
        d->irq_update(d->dma_ctx, 0);
    }
}

void aether_accel_init(AetherAccelDev *d)
{
    memset(d->bar, 0, sizeof(d->bar));
    d->seq = 1;
    d->dev_cursor = 0;
    store32(d, AETHER_REG_MAGIC, AETHER_ACCEL_MAGIC);
    store32(d, AETHER_REG_VERSION, AETHER_ACCEL_VERSION);
    store32(d, AETHER_REG_QSIZE, AETHER_ACCEL_QSIZE);
    store32(d, AETHER_REG_STATUS, 0);
}

uint32_t aether_accel_read32(AetherAccelDev *d, uint32_t off)
{
    return load32(d, off);
}

static int dma_load_i32(AetherAccelDev *d, uint64_t gpa, int32_t *out)
{
    int32_t v;
    if (!d->dma_read) {
        return -1;
    }
    if (d->dma_read(d->dma_ctx, gpa, &v, sizeof(v)) != 0) {
        return -1;
    }
    *out = v;
    return 0;
}

static int dma_store_i32(AetherAccelDev *d, uint64_t gpa, int32_t v)
{
    if (!d->dma_write) {
        return -1;
    }
    return d->dma_write(d->dma_ctx, gpa, &v, sizeof(v));
}

static int check_shape(const AetherJobWire *job)
{
    if (job->m == 0 || job->n == 0 || job->k == 0) {
        return -1;
    }
    if (job->m > 64 || job->n > 64 || job->k > 64) {
        return -1;
    }
    return 0;
}

static int elem_addr(uint64_t base, uint32_t index, uint64_t *out)
{
    uint64_t off = (uint64_t)index * 4u;
    if (base > UINT64_MAX - off) {
        return -1;
    }
    *out = base + off;
    return 0;
}

/* SoftNPU I32 matmul / wave. Overflow and DMA faults become used-ring errors. */
static int softnpu_i32(AetherAccelDev *d, const AetherJobWire *job)
{
    uint32_t i, j, kk;
    int wave = job->op == AETHER_OP_WAVE;

    if (check_shape(job) != 0) {
        return AETHER_CPL_EXEC;
    }
    for (i = 0; i < job->m; i++) {
        for (j = 0; j < job->n; j++) {
            int64_t acc = 0;
            int32_t cv;
            uint64_t c_addr;
            for (kk = 0; kk < job->k; kk++) {
                uint64_t a_addr, b_addr;
                int32_t av, bv;
                int64_t prod;
                if (elem_addr(job->a, i * job->a_stride + kk, &a_addr) != 0
                    || elem_addr(job->b, kk * job->b_stride + j, &b_addr) != 0) {
                    return AETHER_CPL_EXEC;
                }
                if (dma_load_i32(d, a_addr, &av) != 0
                    || dma_load_i32(d, b_addr, &bv) != 0) {
                    return AETHER_CPL_DMA;
                }
                if (__builtin_mul_overflow((int64_t)av, (int64_t)bv, &prod)
                    || __builtin_add_overflow(acc, prod, &acc)) {
                    return AETHER_CPL_EXEC;
                }
            }
            if (wave && job->bias != 0) {
                uint64_t bias_addr;
                int32_t bias;
                if (elem_addr(job->bias, j, &bias_addr) != 0) {
                    return AETHER_CPL_EXEC;
                }
                if (dma_load_i32(d, bias_addr, &bias) != 0) {
                    return AETHER_CPL_DMA;
                }
                if (__builtin_add_overflow(acc, (int64_t)bias, &acc)) {
                    return AETHER_CPL_EXEC;
                }
            }
            if (acc < (int64_t)INT32_MIN || acc > (int64_t)INT32_MAX) {
                return AETHER_CPL_EXEC;
            }
            cv = (int32_t)acc;
            if (elem_addr(job->c, i * job->c_stride + j, &c_addr) != 0) {
                return AETHER_CPL_EXEC;
            }
            if (dma_store_i32(d, c_addr, cv) != 0) {
                return AETHER_CPL_DMA;
            }
        }
    }
    return AETHER_CPL_OK;
}

static int execute_job(AetherAccelDev *d, const AetherJobWire *job)
{
    switch (job->op) {
    case AETHER_OP_NOP:
        return AETHER_CPL_OK;
    case AETHER_OP_MATMUL:
    case AETHER_OP_WAVE:
        if (job->dtype != AETHER_DTYPE_I32) {
            /* F16/F32 stay the in-kernel SoftNPU (path B). */
            return AETHER_CPL_EXEC;
        }
        return softnpu_i32(d, job);
    default:
        return AETHER_CPL_EXEC;
    }
}

static void write_used(AetherAccelDev *d, uint32_t slot, uint32_t seq, int32_t status,
                       uint32_t cycles)
{
    uint32_t off = AETHER_USED_BASE + slot * AETHER_USED_WIRE_SIZE;
    AetherUsedWire u;
    memset(&u, 0, sizeof(u));
    u.job_seq = seq;
    u.status = status;
    u.cycles = cycles;
    if ((uint64_t)off + AETHER_USED_WIRE_SIZE <= AETHER_ACCEL_BAR_SIZE) {
        memcpy(d->bar + off, &u, AETHER_USED_WIRE_SIZE);
    }
}

static void take_one(AetherAccelDev *d, uint32_t token)
{
    uint32_t slot = token % AETHER_ACCEL_QSIZE;
    uint32_t off = AETHER_AVAIL_BASE + slot * AETHER_JOB_WIRE_SIZE;
    AetherJobWire job;
    int status;
    uint32_t cycles;
    uint32_t used;
    uint32_t seq;

    memset(&job, 0, sizeof(job));
    if ((uint64_t)off + AETHER_JOB_WIRE_SIZE <= AETHER_ACCEL_BAR_SIZE) {
        memcpy(&job, d->bar + off, AETHER_JOB_WIRE_SIZE);
    }

    status = execute_job(d, &job);
    {
        uint64_t cyc = (uint64_t)job.m * (uint64_t)job.n * (uint64_t)job.k;
        cycles = cyc > UINT32_MAX ? UINT32_MAX : (uint32_t)cyc;
        if (cycles == 0) {
            cycles = 1;
        }
    }
    seq = d->seq++;
    used = load32(d, AETHER_REG_USED_IDX);
    write_used(d, used % AETHER_ACCEL_QSIZE, seq, status, cycles);
    store32(d, AETHER_REG_USED_IDX, used + 1);
    raise_irq(d);
}

void aether_accel_service(AetherAccelDev *d)
{
    uint32_t avail;

    if (load32(d, AETHER_REG_DOORBELL) == 0) {
        return;
    }
    avail = load32(d, AETHER_REG_AVAIL_IDX);
    while (d->dev_cursor < avail) {
        uint32_t token = d->dev_cursor++;
        take_one(d, token);
    }
    store32(d, AETHER_REG_DOORBELL, 0);
}

void aether_accel_write32(AetherAccelDev *d, uint32_t off, uint32_t val)
{
    switch (off) {
    case AETHER_REG_MAGIC:
    case AETHER_REG_VERSION:
    case AETHER_REG_QSIZE:
        /* ROM-ish: guest cannot rewrite the published identity. */
        return;
    case AETHER_REG_IRQ_ACK:
        if (val != 0) {
            lower_irq(d);
        }
        store32(d, off, val);
        return;
    case AETHER_REG_DOORBELL:
        store32(d, off, val);
        if (val != 0) {
            aether_accel_service(d);
        }
        return;
    default:
        store32(d, off, val);
        return;
    }
}
