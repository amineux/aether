# Error-coverage allowlist: the 9 entries, one page

As of `main` fd5e959 (Round 31, 2026-10-16). Aether has **152** `*Error`
variants in core/hal/drivers/kernel. **143** are named by a host test, demo or
red-team line. The other **9** sit in
[`core/tests/error_coverage_allowlist.txt`](../../core/tests/error_coverage_allowlist.txt),
each with a reason. `core/tests/error_coverage.rs` fails CI if a variant is
neither named nor listed, and also if a listed line goes stale.

None of the 9 is a tenant refuse path waiting for a test. This page says why
each one is there and what would take it off the list.

Reproduce the count:

```sh
cargo test -q --locked -p aether-core --test error_coverage -- --nocapture | grep variants=
```

| Variant | Defined in | Why it is listed | What would retire it |
| --- | --- | --- | --- |
| `SysError::Inval` | `kernel/src/syscall.rs` | Syscall errno. `kernel/` is excluded from the host workspace (`Cargo.toml` `exclude`), so no host test can name it; it runs only on QEMU boot paths. | A QEMU-side test that asserts the errno by name, or moving errno mapping into a host-tested crate. Neither is planned. |
| `SysError::NoCap` | `kernel/src/syscall.rs` | Same: kernel errno, returned when a cap lookup fails in `kernel/src/world.rs`. The host-side refusals underneath it (`CapError::*`) are named. | Same as above. |
| `SysError::Fault` | `kernel/src/syscall.rs` | Same: kernel errno, QEMU-only. | Same as above. |
| `SysError::Again` | `kernel/src/syscall.rs` | Same: kernel errno, returned on `clone`, `mmap` first-fit and `FabricError::QueueFull`. `QueueFull` itself is named on the host. | Same as above. |
| `CapError::GenerationMismatch` | `core/src/caps.rs` | Never constructed. Changes to `CapError` are on the kill list, and open PR #222 touches cap revocation, so this stays as-is. | An owner decision to delete it (an API change) or to wire generation checks; not a needle. |
| `RelocError::BadMagic` | `core/src/reloc.rs` | PIE relocation-pack parser at boot. `RelocError` is on the kill list. | Lifting the kill list for the reloc parser. Not planned. |
| `RelocError::Truncated` | `core/src/reloc.rs` | Same parser, same kill list. | Same as above. |
| `BootFsError::NoRoom` | `core/src/bootfs.rs` | AETHFS01 packer: output buffer too small. Boot image tooling, not a tenant refuse path. | Open PR #230 tests it and removes the line. If #230 merges, the list drops to 8. |
| `OpKernelError::Cap` | `core/src/opkernel.rs` | Dead, kept for ABI. `OpKernelError` is a public `aether_core` return type, but `require_opkernel_bind` refuses every lookup failure as `NotBound` first, so this wrapper cannot fire. Host test `missing_or_out_of_range_cptr_is_not_bound` pins that. | Removing the wrapper, which is a public API change and an owner call. |

## What this page does not claim

- That the kernel errno paths are tested. They run on QEMU boot paths in CI, but
  no test asserts each errno by name.
- That an allowlisted variant is harmless. It means no evidence names it yet.
- Anything about performance, silicon or hardware isolation. See
  [SCOREBOARD.md](../SCOREBOARD.md) for what is and is not measured.

The needle list is closed after Round 31. New coverage needs new refuse paths,
not more tests against the 9 above.
