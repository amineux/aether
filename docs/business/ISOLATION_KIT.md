# Isolation kit: bring your own backend

The isolation kit (`examples/isolation-kit`) runs 15 named attack classes
against a backend and prints, per class, `refused`, `ACCEPTED` or `n/a`, plus
one summary line. This page is the step-by-step for a runtime team that wants
to run it against its own stack. It is a host software harness: not
certification, not a partner result, and no runtime has been integrated yet.
The template below is a starting point, not a result.

## 1. Run the kit as shipped

```
make isolation-matrix
# without GNU make:
cargo run --manifest-path examples/isolation-kit/Cargo.toml --release --quiet
```

You get two matrices: `aether-soft` (the reference adapter, `accepted=0`,
`result=conformant`) and `weak-sample-example-only` (a deliberately weak
teaching stub, `accepted=10`, `result=NONCONFORMANT`). The weak sample is
there to show the kit can fail.

`make isolation-matrix BACKEND=aether-soft` prints three rows side by side:
`aether-soft` (refused=15, accepted=0, n/a=0), `weak-sample-example-only`
(refused=4, accepted=10, n/a=1) and `template-unimplemented` (n/a=15). The
`aether-soft` row runs each class against Aether's own software models (Soft
SMMU windows and spaces, the on-package fabric model, tensor arenas, KV grants,
op-kernel and accelerator admission). Every one of its 15 refusals is an
existing named attack in `examples/red-team/refusal-catalogue.tsv`; a kit test
pins the counts and checks each name, and no new error variant was added. This
is a host software check of those models, not hardware isolation and not a
measurement of any real runtime. A class Aether's models could not refuse would
print `ACCEPTED`, and one that does not apply would print `n/a`.

## 2. Run the empty template

```
make isolation-matrix BACKEND=template
# without GNU make:
cargo run --manifest-path examples/isolation-kit/Cargo.toml --release --quiet -- template
```

This adds a third matrix for `template-unimplemented`: 15 rows of `n/a` and

```
[isolation-kit] backend=template-unimplemented classes=15 applicable=0 refused=0 accepted=0 n/a=15 result=NONCONFORMANT
```

`NONCONFORMANT` is deliberate: a backend with nothing tested does not pass.

## 3. Copy the template

The template is `examples/isolation-kit/template/backend.rs`. Each of the 15
classes has its own `match` arm with a comment saying what the attack is and
what your stack needs in order to refuse it. Copy the file into your crate
(depend on `aether-isolation-kit` by path), or edit it in place, and rename
`TemplateBackend` and its `name()`.

## 4. Fill in one class at a time

For each arm, call your runtime so that it attempts the described attack, then
return:

- `Outcome::Refused` only if your stack returned an error **and** left the
  other tenant's state unchanged (no partial write, no quota charged);
- `Outcome::Accepted` if the attack went through (a real gap, which is what
  the kit is for);
- `Outcome::NotApplicable`, or `false` from `applies`, only for a surface you
  genuinely do not have (for example no KV-cache sharing).

The `expected-error` column shows the refusal Aether's reference adapter
returns. Your stack can use its own error type; only the outcome is scored.

## 5. Run your backend

Call `Matrix::run(&YourBackend)` and print `matrix.table()` and
`matrix.summary_line()`, or add your backend to `main.rs` next to the template.
A backend is `result=conformant` when at least one class applies and no
applicable class is accepted.

## 6. Share the result

Keep the summary line and the table together with the commit SHA of the kit
you ran (`make evidence-pack` bundles the shipped matrices with the SHA). A
conformant matrix means your backend refused these 15 host-modelled attack
classes as you wired them. It is not a hardware-isolation guarantee, and it
covers only the classes listed.

## What the 15 classes cover

| Aspect | Classes |
| --- | --- |
| command | `opkernel-class-mismatch`, `hodge-quota` |
| accelerator | `accel-shape-overflow`, `accel-unsupported-dtype` |
| smmu | `map-user-phys`, `space-not-mappable`, `typed-window-sid`, `smmu-bad-range` |
| fabric | `fabric-queue-full`, `fabric-recv-foreign`, `fabric-send-no-cap`, `fabric-quota-drain` |
| arena | `arena-not-owner`, `arena-limit-leak` |
| kv | `kv-insufficient-rights` |
