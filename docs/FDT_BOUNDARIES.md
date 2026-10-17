# Firmware blob block boundaries

The boot memory-map parser in `core/src/fdt.rs` admits only disjoint DTB
blocks. In addition to checking each offset and length against `totalsize`,
`parse_header` refuses overlap between the structure block, strings block,
and the reservation block's minimum 16-byte extent. It works from the
40-byte header alone; it does not read block contents.

The reservation block has no declared length. The full parser therefore
bounds its scan by the next nonempty structure or strings block, or by
`totalsize` when neither follows it. Each complete reservation entry,
including the `(0, 0)` terminator, must fit before that boundary. An
unterminated block returns `Truncated` rather than borrowing another
block's bytes. Adjacent blocks and disjoint reordered layouts remain
accepted.

Property length/name headers must fit entirely inside the structure block.
The final `FDT_END` must coincide with its declared end; bytes following an
early END are refused as `BadToken` instead of silently ignored. These
checks follow the [DTB format specification, sections 5.2–5.5](https://devicetree-specification.readthedocs.io/en/stable/flattened-format.html).

Host regressions cover all three overlapping block pairs, header-only
validation, touching boundaries, reordered blocks, missing reservation
terminators before both block types, partial property headers, and bytes
after END. One fixture puts an unused zero-filled prefix in the strings
block: it is valid with a proper reservation terminator, but must be
refused if that prefix would supply a missing terminator.

Run `cargo test -p aether-core fdt::tests --locked`. The existing mutation
sweep and firmware refusal demo also run in that test group. CI additionally
boots the existing RISC-V and AArch64 firmware paths.

This is bounded host-tested parser validation, not formal verification or
a tenant isolation claim. No syscall, HAL packet, dependency, unsafe block,
or kernel fallback policy changes. Existing omissions such as `status`
and dynamic reserved pools remain separate work.
