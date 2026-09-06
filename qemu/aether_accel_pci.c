/* SPDX-License-Identifier: MIT OR Apache-2.0
 *
 * QEMU softmmu PCI device wrapping the portable path-A BAR.
 *
 * Drop this file plus aether_accel.{c,h} into hw/misc/ and add the
 * meson.snippet line. See qemu/README.md.
 *
 * Not an upstream virtio device. Vendor 0xAE7E / device 0xACC1.
 * QEMU 8.2 (class_init void *data) and 9.x (const void *data) both
 * build: the prototype matches whatever qemu/typedefs.h provides via
 * a thin wrapper.
 */

#include "qemu/osdep.h"
#include "hw/pci/pci.h"
#include "hw/qdev-properties.h"
#include "qemu/module.h"
#include "qom/object.h"
#include "migration/vmstate.h"

#if __has_include("system/dma.h")
#include "system/dma.h"
#else
#include "sysemu/dma.h"
#endif

#include "aether_accel.h"

#define TYPE_AETHER_ACCEL "aether-accel"
OBJECT_DECLARE_SIMPLE_TYPE(AetherAccelState, AETHER_ACCEL)

struct AetherAccelState {
    PCIDevice parent_obj;
    MemoryRegion bar;
    AetherAccelDev dev;
};

static int aether_pci_dma_read(void *ctx, uint64_t gpa, void *buf, size_t len)
{
    PCIDevice *pdev = ctx;
    return pci_dma_read(pdev, gpa, buf, len) != MEMTX_OK;
}

static int aether_pci_dma_write(void *ctx, uint64_t gpa, const void *buf, size_t len)
{
    PCIDevice *pdev = ctx;
    return pci_dma_write(pdev, gpa, buf, len) != MEMTX_OK;
}

static void aether_pci_irq(void *ctx, int level)
{
    pci_set_irq(ctx, level);
}

static uint64_t aether_mmio_read(void *opaque, hwaddr addr, unsigned size)
{
    AetherAccelState *s = opaque;
    if (size != 4 || (addr & 3) != 0) {
        return 0;
    }
    return aether_accel_read32(&s->dev, (uint32_t)addr);
}

static void aether_mmio_write(void *opaque, hwaddr addr, uint64_t val, unsigned size)
{
    AetherAccelState *s = opaque;
    if (size != 4 || (addr & 3) != 0) {
        return;
    }
    aether_accel_write32(&s->dev, (uint32_t)addr, (uint32_t)val);
}

static const MemoryRegionOps aether_mmio_ops = {
    .read = aether_mmio_read,
    .write = aether_mmio_write,
    .endianness = DEVICE_LITTLE_ENDIAN,
    .valid = {
        .min_access_size = 4,
        .max_access_size = 4,
    },
    .impl = {
        .min_access_size = 4,
        .max_access_size = 4,
    },
};

static void aether_accel_realize(PCIDevice *pci_dev, Error **errp)
{
    AetherAccelState *s = AETHER_ACCEL(pci_dev);

    (void)errp;
    pci_config_set_interrupt_pin(pci_dev->config, 1);
    memory_region_init_io(&s->bar, OBJECT(s), &aether_mmio_ops, s,
                          "aether-accel-bar", AETHER_ACCEL_BAR_SIZE);
    pci_register_bar(pci_dev, 0, PCI_BASE_ADDRESS_SPACE_MEMORY, &s->bar);

    aether_accel_init(&s->dev);
    s->dev.dma_ctx = pci_dev;
    s->dev.dma_read = aether_pci_dma_read;
    s->dev.dma_write = aether_pci_dma_write;
    s->dev.irq_update = aether_pci_irq;
}

static void aether_accel_class_init_inner(ObjectClass *klass)
{
    DeviceClass *dc = DEVICE_CLASS(klass);
    PCIDeviceClass *k = PCI_DEVICE_CLASS(klass);

    k->realize = aether_accel_realize;
    k->vendor_id = AETHER_PCI_VENDOR;
    k->device_id = AETHER_PCI_DEVICE;
    k->revision = 1;
    k->class_id = AETHER_PCI_CLASS;
    set_bit(DEVICE_CATEGORY_MISC, dc->categories);
    dc->desc = "Aether virtio-accel BAR (path A; SoftNPU I32)";
    dc->user_creatable = true;
}

/*
 * QEMU 8.x: class_init(ObjectClass *, void *).
 * QEMU 9.x: class_init(ObjectClass *, const void *).
 * A single function with no second-parameter use compiles as either.
 */
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wcast-function-type"
static void aether_accel_class_init(ObjectClass *klass, void *data)
{
    (void)data;
    aether_accel_class_init_inner(klass);
}
#pragma GCC diagnostic pop

static const TypeInfo aether_accel_info = {
    .name = TYPE_AETHER_ACCEL,
    .parent = TYPE_PCI_DEVICE,
    .instance_size = sizeof(AetherAccelState),
    .class_init = (ObjectClassInitFunc)aether_accel_class_init,
    .interfaces = (InterfaceInfo[]){
        { INTERFACE_CONVENTIONAL_PCI_DEVICE },
        { },
    },
};

static void aether_accel_register_types(void)
{
    type_register_static(&aether_accel_info);
}

type_init(aether_accel_register_types);
