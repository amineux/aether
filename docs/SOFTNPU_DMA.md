# SoftNPU DMA descriptor preflight

`AccelJobDesc::dma_spans` computes conservative contiguous extents for the
operands a job actually uses. For a matrix with `rows`, `cols`, element width
`es` and element stride `stride`, the extent is
`((rows - 1) * stride + cols) * es`, including padding between rows. Shapes,
natural alignment and checked end addresses are validated first. Wave includes
its optional `n * es` bias vector. Relu does not read B; Nop touches no buffers.
Overlapping/broadcast rows remain supported. Packed `bytes_a/b/c` helpers retain
their original meaning; they are not strided DMA bounds.

`SoftNpuDevice` uses these spans both to select operands for PA/IOVA translation
and to check the queued job against existing software SMMU pins before calling
the memory view. An unmapped stride gap/tail or short bias pin completes with
status `-2`, without a DMA read/write or a retired NPU job. A corrected job can
execute afterward. Direct `SoftNpu::execute` also preflights descriptor arithmetic
and alignment before memory access; its caller still owns authorization and
backing-memory validation. Row indices widen to u64 before stride multiplication
so a large stride cannot wrap a u32 index in debug or release builds.

This closes a concrete mismatch: the old driver admitted packed byte counts,
but execution followed row strides and the bias vector. A host regression with
separate operand pins made that old path complete successfully while reading
beyond a pin. Counting memory views now require zero calls on refusal for I32,
F32 and F16; valid exact-boundary strided layouts still execute and preserve
padding. Core tests also check every operand's alignment/address overflow,
wide stride indices and unused operands.

The descriptor layout, frozen packets, syscall numbers and SMMU policy do not
change. These are software checks, not hardware isolation or a complete tenant
security argument. Other backends have independent admission/packet contracts;
this change validates the SoftNPU device and direct reference executor. It does
not make arithmetic-fault execution transactional or roll back already-written
results after a runtime computation error. Kernel atomic batch integration still
requires call-chain and interrupt stack accounting.

```bash
cargo test --locked -p aether-core --test accel_dma
cargo test --locked -p aether-drivers --test softnpu_dma
```
