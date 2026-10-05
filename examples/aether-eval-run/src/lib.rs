//! Host-only evaluation report a design partner can run locally.
//!
//! One shared [`SoftCommandProcessor`](aether_drivers::SoftCommandProcessor)
//! (the software NPU command processor) runs two tenants on one
//! SoftGreenCtx SM/WQ pool: solo, partitioned 70/30, and unpartitioned.
//! Bandwidths are the integer milli units from
//! [`run_greenctx_demo`](aether_core::greenctx::run_greenctx_demo).
//! They are not FLOPs and not a comparison against hardware MIG.
//!
//! Named refusals call existing library demos. This crate does not add
//! an isolation mechanism.

use aether_core::accel::SliceMem;
use aether_core::greenctx::{run_greenctx_demo, GreenCtxReport, MemcpyReport, DEMO_MEMCPY_BYTES};
use aether_core::iommu::{run_smmu_unmap_cross_tenant_demo, run_smmu_wrong_stream_demo, StreamId};
use aether_core::types::{ChipletId, PhysAddr, TileId};
use aether_drivers::{run_firewall_demo, SoftCommandProcessor, CP_SSID};

/// What this run does not establish. Copied into the JSON `claims` block.
pub const NOT_PROVEN: &[&str] = &[
    "no hardware SMMU, silicon, or device DMA enforcement",
    "no customer, design win, or paid pilot",
    "no performance superiority versus MIG or any hardware partition",
    "no FLOPs, tape-out, or accelerator-throughput claim",
];

/// Toolchain and tree identity recorded with the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvMeta {
    pub git_commit: Option<String>,
    pub git_dirty: Option<bool>,
    pub rustc_version: Option<String>,
    pub cargo_version: Option<String>,
    pub cargo_lock_sha256: Option<String>,
}

impl EnvMeta {
    fn complete(&self) -> bool {
        self.git_commit.as_ref().is_some_and(|s| !s.is_empty())
            && self.rustc_version.as_ref().is_some_and(|s| !s.is_empty())
            && self.cargo_version.as_ref().is_some_and(|s| !s.is_empty())
            && self
                .cargo_lock_sha256
                .as_ref()
                .is_some_and(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MilliPoint {
    bw_milli: u32,
    interference_milli: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TenantMilli {
    solo: MilliPoint,
    partitioned_70_30: MilliPoint,
    unpartitioned: MilliPoint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Workload {
    demo_ok: bool,
    shared_cp_matches: bool,
    split_ok: bool,
    interference_ok: bool,
    not_mig: bool,
    migrate_ok: bool,
    tenant_a: TenantMilli,
    tenant_b: TenantMilli,
}

impl Workload {
    fn ok(&self) -> bool {
        self.demo_ok
            && self.shared_cp_matches
            && self.split_ok
            && self.interference_ok
            && self.not_mig
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Refusal {
    name: &'static str,
    passed: bool,
    error: &'static str,
}

/// Machine-readable evaluation. `passed` is the conjunction of complete
/// metadata, the SoftGreenCtx milli checks, and every named refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvalReport {
    pub passed: bool,
    env: EnvMeta,
    workload: Workload,
    refusals: Vec<Refusal>,
}

/// Run the shared Soft-CP / SoftGreenCtx workload and the named refusals.
pub fn evaluate(env: &EnvMeta) -> EvalReport {
    let workload = measure_workload();
    let refusals = measure_refusals();
    let passed = env.complete() && workload.ok() && refusals.iter().all(|r| r.passed);
    EvalReport {
        passed,
        env: env.clone(),
        workload,
        refusals,
    }
}

fn measure_refusals() -> Vec<Refusal> {
    let unmap = run_smmu_unmap_cross_tenant_demo();
    let wrong = run_smmu_wrong_stream_demo();
    let firewall = run_firewall_demo();
    vec![
        Refusal {
            name: "smmu-unmap-cross-tenant",
            passed: unmap.all_ok(),
            error: "MapError::CrossTenant",
        },
        Refusal {
            name: "smmu-wrong-stream",
            passed: wrong.all_ok(),
            error: "MapError::WrongStream",
        },
        Refusal {
            name: "softcmdfirewall",
            passed: firewall.all_ok(),
            error: "copy-then-validate holds; in-place mutation does not",
        },
    ]
}

fn measure_workload() -> Workload {
    let demo = run_greenctx_demo();
    let solo = shared_solo();
    let unpart = shared_pair(false);
    let part = shared_pair(true);
    let shared_cp_matches = match (solo, unpart, part) {
        (Some(solo), Some((ua, ub)), Some((p70, p30))) => {
            cp_matches_demo(&demo, solo, ua, ub, p70, p30)
        }
        _ => false,
    };
    let solo_pt = MilliPoint {
        bw_milli: demo.solo_bw,
        interference_milli: 0,
    };
    Workload {
        demo_ok: demo.all_ok(),
        shared_cp_matches,
        split_ok: demo.split_ok,
        interference_ok: demo.interference_ok,
        not_mig: demo.not_mig,
        migrate_ok: demo.migrate_ok,
        tenant_a: TenantMilli {
            solo: solo_pt,
            partitioned_70_30: MilliPoint {
                bw_milli: demo.part70_bw,
                interference_milli: demo.part70_interference,
            },
            unpartitioned: MilliPoint {
                bw_milli: demo.unpart_bw,
                interference_milli: demo.unpart_interference,
            },
        },
        tenant_b: TenantMilli {
            solo: solo_pt,
            partitioned_70_30: MilliPoint {
                bw_milli: demo.part30_bw,
                interference_milli: demo.part30_interference,
            },
            unpartitioned: MilliPoint {
                bw_milli: demo.unpart_bw,
                interference_milli: demo.unpart_interference,
            },
        },
    }
}

fn cp_matches_demo(
    demo: &GreenCtxReport,
    solo: MemcpyReport,
    ua: MemcpyReport,
    ub: MemcpyReport,
    p70: MemcpyReport,
    p30: MemcpyReport,
) -> bool {
    solo.bw_milli == demo.solo_bw
        && ua.bw_milli == demo.unpart_bw
        && ub.bw_milli == demo.unpart_bw
        && p70.bw_milli == demo.part70_bw
        && p30.bw_milli == demo.part30_bw
        && ua.interference_milli(solo) == demo.unpart_interference
        && ub.interference_milli(solo) == demo.unpart_interference
        && p70.interference_milli(solo) == demo.part70_interference
        && p30.interference_milli(solo) == demo.part30_interference
}

fn shared_solo() -> Option<MemcpyReport> {
    let mut backing = [0u8; 16];
    let mem = SliceMem {
        base: PhysAddr(0),
        bytes: &mut backing,
    };
    let mut dev = SoftCommandProcessor::new(mem);
    let sid_a = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
    dev.create_xqueue(0, sid_a, 0).ok()?;
    let (ctx, _) = dev.share_green_unpartitioned().ok()?;
    dev.bind_green_ctx(0, ctx).ok()?;
    dev.submit_memcpy(0, DEMO_MEMCPY_BYTES).ok()?;
    let cpl = dev.service()?;
    if cpl.status != 0 {
        return None;
    }
    dev.last_memcpy()
}

/// Two tenants on one Soft-CP. Queue 0 is tenant A (70% when partitioned).
/// Queue 1 is tenant B (30% when partitioned).
fn shared_pair(partitioned: bool) -> Option<(MemcpyReport, MemcpyReport)> {
    let mut backing = [0u8; 16];
    let mem = SliceMem {
        base: PhysAddr(0),
        bytes: &mut backing,
    };
    let mut dev = SoftCommandProcessor::new(mem);
    let sid_a = StreamId::accel(ChipletId(0), TileId(2), CP_SSID);
    let sid_b = StreamId::accel(ChipletId(1), TileId(3), CP_SSID);
    dev.create_xqueue(0, sid_a, 0).ok()?;
    dev.create_xqueue(1, sid_b, 0).ok()?;
    let (ctx_a, ctx_b) = if partitioned {
        dev.split_green_70_30().ok()?
    } else {
        dev.share_green_unpartitioned().ok()?
    };
    dev.bind_green_ctx(0, ctx_a).ok()?;
    dev.bind_green_ctx(1, ctx_b).ok()?;
    dev.submit_memcpy(0, DEMO_MEMCPY_BYTES).ok()?;
    dev.submit_memcpy(1, DEMO_MEMCPY_BYTES).ok()?;
    let first = dev.service()?;
    if first.status != 0 {
        return None;
    }
    let a = dev.last_memcpy()?;
    let second = dev.service()?;
    if second.status != 0 {
        return None;
    }
    let b = dev.last_memcpy()?;
    Some((a, b))
}

impl EvalReport {
    /// Pretty-printed JSON. Field names are the contract in `docs/business/EVAL_RUN.md`.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        out.push_str("{\n");
        push_str(&mut out, 1, "schema", "aether-eval-run/v1");
        push_bool(&mut out, 1, "passed", self.passed);
        push_opt_str(&mut out, 1, "git_commit", &self.env.git_commit);
        push_opt_bool(&mut out, 1, "git_dirty", self.env.git_dirty);
        push_opt_str(&mut out, 1, "rustc_version", &self.env.rustc_version);
        push_opt_str(&mut out, 1, "cargo_version", &self.env.cargo_version);
        push_opt_str(
            &mut out,
            1,
            "cargo_lock_sha256",
            &self.env.cargo_lock_sha256,
        );
        out.push_str("  \"seed\": null,\n");
        self.push_workload(&mut out);
        self.push_refusals(&mut out);
        self.push_claims(&mut out);
        out.push_str("}\n");
        out
    }

    fn push_workload(&self, out: &mut String) {
        let w = &self.workload;
        out.push_str("  \"workload\": {\n");
        push_str(out, 2, "name", "soft-greenctx-memcpy");
        push_str(
            out,
            2,
            "shared_device",
            "one SoftCommandProcessor (software NPU command processor) with two tenants on one SoftGreenCtx SM/WQ pool",
        );
        push_str(
            out,
            2,
            "units",
            "integer milli; 1000 = solo full-pool memcpy bandwidth; not FLOPs",
        );
        push_str(
            out,
            2,
            "source",
            "run_greenctx_demo cross-checked on SoftCommandProcessor submit_memcpy",
        );
        push_bool(out, 2, "demo_ok", w.demo_ok);
        push_bool(out, 2, "shared_cp_matches", w.shared_cp_matches);
        push_bool(out, 2, "split_ok", w.split_ok);
        push_bool(out, 2, "interference_ok", w.interference_ok);
        push_bool(out, 2, "not_mig", w.not_mig);
        push_bool(out, 2, "migrate_ok", w.migrate_ok);
        out.push_str("    \"tenants\": {\n");
        push_tenant(out, "A", &w.tenant_a, "70");
        push_tenant(out, "B", &w.tenant_b, "30");
        // drop trailing comma on the last tenant object by rewriting is harder;
        // both tenants are emitted with commas except we close cleanly below.
        // push_tenant always ends with a comma after the object. Trim the last one.
        trim_trailing_comma(out);
        out.push_str("\n    }\n");
        out.push_str("  },\n");
    }

    fn push_refusals(&self, out: &mut String) {
        out.push_str("  \"refusals\": [\n");
        for (i, r) in self.refusals.iter().enumerate() {
            out.push_str("    {\n");
            push_str(out, 3, "name", r.name);
            push_bool(out, 3, "passed", r.passed);
            push_str_last(out, 3, "error", r.error);
            out.push_str("    }");
            if i + 1 != self.refusals.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("  ],\n");
    }

    fn push_claims(&self, out: &mut String) {
        out.push_str("  \"claims\": {\n");
        out.push_str("    \"not_proven\": [\n");
        for (i, line) in NOT_PROVEN.iter().enumerate() {
            out.push_str("      ");
            out.push_str(&json_string(line));
            if i + 1 != NOT_PROVEN.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("    ]\n");
        out.push_str("  }\n");
    }

    /// Short human summary. The JSON is the machine-readable record.
    pub fn summary(&self) -> String {
        let w = &self.workload;
        let mut s = String::new();
        s.push_str("aether-eval-run  host software model only\n");
        s.push_str(&format!(
            "git {}  dirty={}\n",
            disp_opt(&self.env.git_commit),
            disp_opt_bool(self.env.git_dirty)
        ));
        s.push_str(&format!("{}\n", disp_opt(&self.env.rustc_version)));
        s.push_str(&format!("{}\n", disp_opt(&self.env.cargo_version)));
        s.push_str(&format!(
            "Cargo.lock sha256 {}\n",
            disp_opt(&self.env.cargo_lock_sha256)
        ));
        s.push_str("seed none (deterministic SoftGreenCtx memcpy; not the tenant-fuzz seed)\n");
        s.push_str("SoftGreenCtx two-tenant memcpy on one Soft-CP (integer milli, not FLOPs)\n");
        s.push_str(&format!(
            "  solo           A bw={} interference={}   B bw={} interference={}\n",
            w.tenant_a.solo.bw_milli,
            w.tenant_a.solo.interference_milli,
            w.tenant_b.solo.bw_milli,
            w.tenant_b.solo.interference_milli
        ));
        s.push_str(&format!(
            "  70/30          A bw={} interference={}   B bw={} interference={}\n",
            w.tenant_a.partitioned_70_30.bw_milli,
            w.tenant_a.partitioned_70_30.interference_milli,
            w.tenant_b.partitioned_70_30.bw_milli,
            w.tenant_b.partitioned_70_30.interference_milli
        ));
        s.push_str(&format!(
            "  unpartitioned  A bw={} interference={}   B bw={} interference={}\n",
            w.tenant_a.unpartitioned.bw_milli,
            w.tenant_a.unpartitioned.interference_milli,
            w.tenant_b.unpartitioned.bw_milli,
            w.tenant_b.unpartitioned.interference_milli
        ));
        s.push_str(&format!(
            "  shared_cp_matches={} split_ok={} interference_ok={} not_mig={}\n",
            w.shared_cp_matches, w.split_ok, w.interference_ok, w.not_mig
        ));
        s.push_str("refusals (must pass)\n");
        for r in &self.refusals {
            s.push_str(&format!(
                "  {}  {}\n",
                r.name,
                if r.passed { "pass" } else { "FAIL" }
            ));
        }
        s.push_str("not proven:\n");
        for line in NOT_PROVEN {
            s.push_str(&format!("  {line}\n"));
        }
        if self.env.git_dirty == Some(true) {
            s.push_str("note: git_dirty is true, so git_commit does not uniquely name this tree\n");
        }
        s.push_str(&format!(
            "passed={}\n",
            if self.passed { "true" } else { "false" }
        ));
        s
    }
}

fn push_tenant(out: &mut String, name: &str, t: &TenantMilli, slice: &str) {
    out.push_str(&format!("      \"{name}\": {{\n"));
    push_str(out, 4, "partitioned_slice", slice);
    push_point(out, "solo", t.solo);
    push_point(out, "partitioned_70_30", t.partitioned_70_30);
    push_point_last(out, "unpartitioned", t.unpartitioned);
    out.push_str("      },\n");
}

fn push_point(out: &mut String, key: &str, p: MilliPoint) {
    out.push_str(&format!(
        "        \"{key}\": {{\"bw_milli\": {}, \"interference_milli\": {}}},\n",
        p.bw_milli, p.interference_milli
    ));
}

fn push_point_last(out: &mut String, key: &str, p: MilliPoint) {
    out.push_str(&format!(
        "        \"{key}\": {{\"bw_milli\": {}, \"interference_milli\": {}}}\n",
        p.bw_milli, p.interference_milli
    ));
}

fn trim_trailing_comma(out: &mut String) {
    if out.ends_with(",\n") {
        out.pop();
        out.pop();
        // leave the newline that the caller adds, or keep a newline here
        // The caller does `trim` then `\n    }\n`. After pop of `\n` and `,`,
        // the object close `},` lost its comma and newline. Restore newline.
        out.push('\n');
    }
}

fn push_str(out: &mut String, indent: usize, key: &str, value: &str) {
    pad(out, indent);
    out.push_str(&format!("\"{key}\": {},\n", json_string(value)));
}

fn push_str_last(out: &mut String, indent: usize, key: &str, value: &str) {
    pad(out, indent);
    out.push_str(&format!("\"{key}\": {}\n", json_string(value)));
}

fn push_bool(out: &mut String, indent: usize, key: &str, value: bool) {
    pad(out, indent);
    out.push_str(&format!(
        "\"{key}\": {},\n",
        if value { "true" } else { "false" }
    ));
}

fn push_opt_str(out: &mut String, indent: usize, key: &str, value: &Option<String>) {
    pad(out, indent);
    match value {
        Some(v) => out.push_str(&format!("\"{key}\": {},\n", json_string(v))),
        None => out.push_str(&format!("\"{key}\": null,\n")),
    }
}

fn push_opt_bool(out: &mut String, indent: usize, key: &str, value: Option<bool>) {
    pad(out, indent);
    let rendered = match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "null",
    };
    out.push_str(&format!("\"{key}\": {rendered},\n"));
}

fn pad(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn disp_opt(v: &Option<String>) -> String {
    match v {
        Some(s) if !s.is_empty() => s.clone(),
        _ => "missing".into(),
    }
}

fn disp_opt_bool(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "true",
        Some(false) => "false",
        None => "missing",
    }
}

/// SHA-256 of `data`. No extra crate: the lockfile hash is a reproduction field.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).saturating_mul(8);
    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut bytes = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    hex_encode(&bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_env() -> EnvMeta {
        EnvMeta {
            git_commit: Some("abc123".into()),
            git_dirty: Some(false),
            rustc_version: Some("rustc 1.83.0".into()),
            cargo_version: Some("cargo 1.83.0".into()),
            cargo_lock_sha256: Some("ab".repeat(32)),
        }
    }

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn report_passes_refusals_workload_and_claims() {
        let report = evaluate(&sample_env());
        assert!(report.passed, "{}", report.summary());
        assert!(report.workload.shared_cp_matches);
        assert!(report.workload.not_mig);
        assert!(
            report.workload.tenant_a.solo.bw_milli
                > report.workload.tenant_a.partitioned_70_30.bw_milli
        );
        assert!(
            report.workload.tenant_a.partitioned_70_30.bw_milli
                > report.workload.tenant_a.unpartitioned.bw_milli
        );
        assert!(
            report.workload.tenant_a.unpartitioned.bw_milli
                > report.workload.tenant_b.partitioned_70_30.bw_milli
        );
        assert_eq!(
            report.workload.tenant_a.unpartitioned.bw_milli,
            report.workload.tenant_b.unpartitioned.bw_milli
        );
        assert_eq!(report.workload.tenant_a.solo.interference_milli, 0);
        let json = report.to_json();
        assert!(json.contains("\"schema\": \"aether-eval-run/v1\""));
        assert!(json.contains("\"passed\": true"));
        assert!(json.contains("\"seed\": null"));
        assert!(json.contains("\"name\": \"smmu-unmap-cross-tenant\""));
        assert!(json.contains("\"name\": \"smmu-wrong-stream\""));
        assert!(json.contains("\"name\": \"softcmdfirewall\""));
        assert!(json.contains("MapError::CrossTenant"));
        assert!(json.contains("MapError::WrongStream"));
        for line in NOT_PROVEN {
            assert!(json.contains(line), "missing claim: {line}");
        }
        assert!(report.summary().contains("passed=true"));
    }

    #[test]
    fn missing_metadata_fails_even_when_checks_pass() {
        let report = evaluate(&EnvMeta {
            git_commit: None,
            git_dirty: None,
            rustc_version: None,
            cargo_version: None,
            cargo_lock_sha256: None,
        });
        assert!(!report.passed);
        assert!(report.workload.ok());
        assert!(report.refusals.iter().all(|r| r.passed));
        let json = report.to_json();
        assert!(json.contains("\"passed\": false"));
        assert!(json.contains("\"git_commit\": null"));
    }

    #[test]
    fn json_escapes_toolchain_quotes() {
        let mut env = sample_env();
        env.rustc_version = Some("rustc \"nightly\"".into());
        let json = evaluate(&env).to_json();
        assert!(json.contains("rustc \\\"nightly\\\""));
    }
}
