#!/bin/sh
# Copy the path-A softmmu sources into a QEMU tree and register them
# in hw/misc/meson.build. Does not configure or build QEMU.
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
QEMU_SRC=${QEMU_SRC:-${1:-}}

if [ -z "$QEMU_SRC" ] || [ ! -d "$QEMU_SRC/hw/misc" ]; then
    echo "usage: QEMU_SRC=/path/to/qemu $0" >&2
    echo "       $0 /path/to/qemu" >&2
    exit 2
fi

DEST="$QEMU_SRC/hw/misc"
cp -f "$HERE/aether_accel.h" "$HERE/aether_accel.c" "$HERE/aether_accel_pci.c" "$DEST/"

MESON="$DEST/meson.build"
MARK="aether_accel_pci.c"
if grep -q "$MARK" "$MESON"; then
    echo "install-into-qemu: $MESON already lists $MARK"
else
    cat >> "$MESON" << 'EOF'

# Aether path-A virtio-accel BAR (optional; not upstream virtio).
softmmu_ss.add(when: 'CONFIG_PCI', if_true: files(
  'aether_accel.c',
  'aether_accel_pci.c',
))
EOF
    echo "install-into-qemu: appended aether-accel to $MESON"
fi

echo "install-into-qemu: copied sources to $DEST"
echo "install-into-qemu: next: cd $QEMU_SRC && ./configure --target-list=x86_64-softmmu && ninja -C build"
echo "install-into-qemu: then: export QEMU_ACCEL=$QEMU_SRC/build/qemu-system-x86_64"
