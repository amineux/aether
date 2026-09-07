//! Soft SMMU bring-up kit: dump/replay of **software** tables.
//!
//! Tied to [`crate::iommu::IommuMap`] (the same tables `AccelDevice::map`
//! and Soft-CP SID bind install). This is **not** a Soft-SMMU redo, **not**
//! a hardware SMMU, and **not** an SMMUv3 emulator.
//!
//! Script: `docs/bringup/smmu_replay.jsonl`. How-to: `docs/bringup/BRINGUP.md`.

use crate::caps::{CapKind, CapRights, Capability};
use crate::iommu::{
    AtcDumpLine, CdTableDump, InvCmd, IommuMap, MapError, MapRequest, MappedRegion, SoftPte,
    SoftSmmuDump, SteTableDump, StreamId, StreamState, WalkResult,
};
use crate::types::{ChipletId, PhysAddr, TenantId, TileId};
use core::fmt::{self, Write};

/// Soft-CP substream (`aether_drivers::fakecp::CP_SSID`). Not a Host1x SID.
pub const KIT_CP_SSID: u8 = 1;
/// IreeShapedCp substream (`IREE_SSID`). M1 pin on the same STE.
pub const KIT_IREE_SSID: u8 = 2;
/// Sibling SSID on the kit STE with no CD (abort / wrong-stream helper).
pub const KIT_WRONG_SSID: u8 = 3;
/// Tile used by the canned Soft-CP bind sequence.
pub const KIT_TILE: u16 = 2;
/// Guest PA the kit pins (4 KiB).
pub const KIT_GUEST_PA: u64 = 0x1000;
/// Pin length (bytes).
pub const KIT_LEN: u64 = 0x1000;

/// Packed Soft-CP SID the kit binds: chiplet 0, tile 2, ssid 1.
pub const fn kit_cp_sid() -> StreamId {
    StreamId::accel(ChipletId(0), TileId(KIT_TILE), KIT_CP_SSID)
}

/// Packed IreeShapedCp SID (M1) on the same STE: ssid 2.
pub const fn kit_iree_sid() -> StreamId {
    StreamId::accel(ChipletId(0), TileId(KIT_TILE), KIT_IREE_SSID)
}

/// Same STE, no CD until bound. Used for abort / wrong-stream steps.
pub const fn kit_wrong_sid() -> StreamId {
    StreamId::accel(ChipletId(0), TileId(KIT_TILE), KIT_WRONG_SSID)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BringupErrorKind {
    BadLine,
    UnknownOp,
    MissingField,
    ExpectMismatch,
    Map(MapError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BringupError {
    pub step: u32,
    pub kind: BringupErrorKind,
}

impl BringupError {
    fn new(step: u32, kind: BringupErrorKind) -> Self {
        Self { step, kind }
    }
}

/// Replay a JSONL script of Soft-SMMU ops against a fresh [`IommuMap`].
///
/// Lines with `"op":"note"` are ignored. Unknown ops fail. Expected
/// outcomes are strings matching [`MapError::as_str`] / [`StreamState::as_str`].
pub fn replay_jsonl(script: &str) -> Result<SoftSmmuDump, BringupError> {
    let mut iommu = IommuMap::new();
    let cap =
        Capability::new(CapKind::Memory, CapRights::MEM_FULL, 1, TenantId(1)).with_generation(1);
    let no_map = Capability::new(
        CapKind::Memory,
        CapRights(CapRights::READ | CapRights::WRITE),
        1,
        TenantId(1),
    )
    .with_generation(1);
    let mut last_iova: Option<PhysAddr> = None;
    let mut step_no = 0u32;

    for raw in script.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        step_no = json_u32(line, "step").unwrap_or(step_no.saturating_add(1));
        let op =
            json_str(line, "op").ok_or(BringupError::new(step_no, BringupErrorKind::BadLine))?;
        if op == "note" {
            continue;
        }
        let sid = parse_sid(line, step_no)?;
        let expect = json_str(line, "expect");
        match op {
            "capture" => {
                let st = iommu
                    .capture(sid)
                    .map_err(|e| BringupError::new(step_no, BringupErrorKind::Map(e)))?;
                expect_state(step_no, expect, st)?;
            }
            "bind_stream" | "bind_nested" => {
                let with_cap = json_bool(line, "with_cap").unwrap_or(true);
                let used = if with_cap { &cap } else { &no_map };
                let res = if op == "bind_nested" {
                    iommu.bind_nested(used, sid)
                } else {
                    iommu.bind_stream(used, sid)
                };
                match (res, expect) {
                    (Ok(st), Some(exp)) => expect_state(step_no, Some(exp), st)?,
                    (Ok(_), None) => {}
                    (Err(e), Some(exp)) => expect_err(step_no, exp, e)?,
                    (Err(e), None) => {
                        return Err(BringupError::new(step_no, BringupErrorKind::Map(e)));
                    }
                }
            }
            "map" => {
                let with_cap = json_bool(line, "with_cap").unwrap_or(true);
                let used = if with_cap { &cap } else { &no_map };
                let pa = json_hex(line, "guest_pa").unwrap_or(KIT_GUEST_PA);
                let len = json_hex(line, "len").unwrap_or(KIT_LEN);
                let req = MapRequest::pin_accel(PhysAddr(pa), len, sid);
                match iommu.map(used, req) {
                    Ok(r) => {
                        last_iova = Some(r.iova);
                        if let Some(exp) = expect {
                            if exp != "ok" {
                                return Err(BringupError::new(
                                    step_no,
                                    BringupErrorKind::ExpectMismatch,
                                ));
                            }
                        }
                        if let Some(min) = json_hex(line, "expect_iova_ge") {
                            if r.iova.0 < min {
                                return Err(BringupError::new(
                                    step_no,
                                    BringupErrorKind::ExpectMismatch,
                                ));
                            }
                        }
                        if json_bool(line, "expect_iova_ne_pa").unwrap_or(false) && r.iova.0 == pa {
                            return Err(BringupError::new(
                                step_no,
                                BringupErrorKind::ExpectMismatch,
                            ));
                        }
                    }
                    Err(e) => match expect {
                        Some(exp) => expect_err(step_no, exp, e)?,
                        None => return Err(BringupError::new(step_no, BringupErrorKind::Map(e))),
                    },
                }
            }
            "translate" => {
                let pa = json_hex(line, "guest_pa")
                    .ok_or(BringupError::new(step_no, BringupErrorKind::MissingField))?;
                match iommu.translate_result(sid.raw(), PhysAddr(pa), None) {
                    Ok(iova) => {
                        if let Some(exp) = expect {
                            if parse_hex(exp) != Some(iova.0) && exp != "ok" {
                                return Err(BringupError::new(
                                    step_no,
                                    BringupErrorKind::ExpectMismatch,
                                ));
                            }
                        }
                    }
                    Err(e) => match expect {
                        Some(exp) => expect_err(step_no, exp, e)?,
                        None => return Err(BringupError::new(step_no, BringupErrorKind::Map(e))),
                    },
                }
            }
            "walk" => {
                let addr = walk_addr(line, last_iova, step_no)?;
                match iommu.walk(sid, addr) {
                    Ok(w) => check_walk(step_no, line, &w)?,
                    Err(e) => match expect {
                        Some(exp) => expect_err(step_no, exp, e)?,
                        None => return Err(BringupError::new(step_no, BringupErrorKind::Map(e))),
                    },
                }
            }
            "resolve_ats" => {
                let iova = walk_addr(line, last_iova, step_no)?;
                match iommu.resolve_ats(sid.raw(), iova) {
                    Ok(pa) => {
                        if let Some(exp_pa) = json_hex(line, "expect_pa") {
                            if pa.0 != exp_pa {
                                return Err(BringupError::new(
                                    step_no,
                                    BringupErrorKind::ExpectMismatch,
                                ));
                            }
                        }
                    }
                    Err(e) => match expect {
                        Some(exp) => expect_err(step_no, exp, e)?,
                        None => return Err(BringupError::new(step_no, BringupErrorKind::Map(e))),
                    },
                }
            }
            "invalidate" => {
                let cmd = parse_inv(line, sid, last_iova, step_no)?;
                let dropped = iommu
                    .invalidate(cmd)
                    .map_err(|e| BringupError::new(step_no, BringupErrorKind::Map(e)))?;
                if let Some(n) = json_u32(line, "expect_dropped") {
                    if dropped != n {
                        return Err(BringupError::new(step_no, BringupErrorKind::ExpectMismatch));
                    }
                }
            }
            "dump" => {}
            _ => return Err(BringupError::new(step_no, BringupErrorKind::UnknownOp)),
        }
    }
    Ok(iommu.dump())
}

fn check_walk(step: u32, line: &str, w: &WalkResult) -> Result<(), BringupError> {
    if let Some(exp) = json_hex(line, "expect_pa") {
        if w.pa.0 != exp {
            return Err(BringupError::new(step, BringupErrorKind::ExpectMismatch));
        }
    }
    if json_bool(line, "expect_ipa_ne_pa").unwrap_or(false) && w.ipa.0 == w.pa.0 {
        return Err(BringupError::new(step, BringupErrorKind::ExpectMismatch));
    }
    if let Some(cfg) = json_str(line, "expect_config") {
        if w.config.as_str() != cfg {
            return Err(BringupError::new(step, BringupErrorKind::ExpectMismatch));
        }
    }
    if let Some(exp) = json_str(line, "expect") {
        if exp != "ok" && parse_hex(exp) != Some(w.pa.0) {
            return Err(BringupError::new(step, BringupErrorKind::ExpectMismatch));
        }
    }
    Ok(())
}

fn expect_state(step: u32, expect: Option<&str>, st: StreamState) -> Result<(), BringupError> {
    if let Some(exp) = expect {
        if st.as_str() != exp {
            return Err(BringupError::new(step, BringupErrorKind::ExpectMismatch));
        }
    }
    Ok(())
}

fn expect_err(step: u32, exp: &str, e: MapError) -> Result<(), BringupError> {
    if e.as_str() != exp {
        return Err(BringupError::new(step, BringupErrorKind::ExpectMismatch));
    }
    Ok(())
}

fn parse_sid(line: &str, step: u32) -> Result<StreamId, BringupError> {
    if let Some(name) = json_str(line, "sid") {
        return match name {
            "cp" => Ok(kit_cp_sid()),
            "iree" => Ok(kit_iree_sid()),
            "wrong" => Ok(kit_wrong_sid()),
            _ => parse_hex(name)
                .map(|v| StreamId::from_raw(v as u32))
                .ok_or(BringupError::new(step, BringupErrorKind::MissingField)),
        };
    }
    Ok(kit_cp_sid())
}

fn walk_addr(line: &str, last_iova: Option<PhysAddr>, step: u32) -> Result<PhysAddr, BringupError> {
    match json_str(line, "addr").or_else(|| json_str(line, "iova")) {
        Some("iova") | Some("mapped") => {
            last_iova.ok_or(BringupError::new(step, BringupErrorKind::MissingField))
        }
        Some(s) => parse_hex(s)
            .map(PhysAddr)
            .ok_or(BringupError::new(step, BringupErrorKind::MissingField)),
        None => last_iova.ok_or(BringupError::new(step, BringupErrorKind::MissingField)),
    }
}

fn parse_inv(
    line: &str,
    sid: StreamId,
    last_iova: Option<PhysAddr>,
    step: u32,
) -> Result<InvCmd, BringupError> {
    let cmd = json_str(line, "cmd").unwrap_or("Ats");
    match cmd {
        "All" | "TlbiAll" => Ok(InvCmd::All),
        "Tlbi" => Ok(InvCmd::Tlbi { sid: Some(sid) }),
        "CfgSte" => Ok(InvCmd::CfgSte { sid }),
        "CfgCd" => Ok(InvCmd::CfgCd { sid }),
        "Ats" => {
            let iova = match json_str(line, "iova") {
                Some("iova") | Some("mapped") | None => last_iova,
                Some(s) => parse_hex(s).map(PhysAddr),
            };
            let len = json_hex(line, "len").unwrap_or(KIT_LEN);
            Ok(InvCmd::Ats { sid, iova, len })
        }
        _ => Err(BringupError::new(step, BringupErrorKind::UnknownOp)),
    }
}

fn json_str<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = match key {
        "op" => "\"op\"",
        "sid" => "\"sid\"",
        "expect" => "\"expect\"",
        "guest_pa" => "\"guest_pa\"",
        "len" => "\"len\"",
        "addr" => "\"addr\"",
        "iova" => "\"iova\"",
        "cmd" => "\"cmd\"",
        "expect_config" => "\"expect_config\"",
        _ => return json_str_dyn(line, key),
    };
    json_after(line, needle)
}

fn json_str_dyn<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut buf = [0u8; 40];
    if key.len() + 2 > buf.len() {
        return None;
    }
    buf[0] = b'"';
    buf[1..1 + key.len()].copy_from_slice(key.as_bytes());
    buf[1 + key.len()] = b'"';
    let needle = core::str::from_utf8(&buf[..key.len() + 2]).ok()?;
    json_after(line, needle)
}

fn json_after<'a>(line: &'a str, needle: &str) -> Option<&'a str> {
    let i = line.find(needle)?;
    let rest = line[i + needle.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    if let Some(s) = rest.strip_prefix('"') {
        let end = s.find('"')?;
        Some(&s[..end])
    } else if rest.starts_with("true") {
        Some("true")
    } else if rest.starts_with("false") {
        Some("false")
    } else {
        let end = rest
            .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
            .unwrap_or(rest.len());
        let tok = rest[..end].trim();
        if tok.is_empty() {
            None
        } else {
            Some(tok)
        }
    }
}

fn json_bool(line: &str, key: &str) -> Option<bool> {
    match json_str(line, key)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn json_hex(line: &str, key: &str) -> Option<u64> {
    json_str(line, key).and_then(parse_hex)
}

fn json_u32(line: &str, key: &str) -> Option<u32> {
    json_str(line, key).and_then(|s| {
        if let Some(h) = parse_hex(s) {
            u32::try_from(h).ok()
        } else {
            s.parse().ok()
        }
    })
}

fn parse_hex(s: &str) -> Option<u64> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok()
    } else {
        t.parse().ok()
    }
}

/// Canonical JSON for a software-table dump. `fmt::Write` so this stays `no_std`.
pub fn write_dump_json<W: Write>(dump: &SoftSmmuDump, w: &mut W) -> fmt::Result {
    w.write_str("{\n")?;
    w.write_str("  \"kind\": \"aether-soft-smmu-dump\",\n")?;
    w.write_str("  \"version\": 1,\n")?;
    w.write_str("  \"software_tables_only\": true,\n")?;
    w.write_str("  \"not_hardware_smmu\": true,\n")?;
    w.write_str("  \"not_smmuv3_emulator\": true,\n")?;
    w.write_str("  \"stes\": [\n")?;
    let mut first_ste = true;
    for (slot, ste) in dump.stes.iter().enumerate() {
        let Some(s) = ste else {
            continue;
        };
        if !first_ste {
            w.write_str(",\n")?;
        }
        first_ste = false;
        write_ste_json(w, slot, s)?;
    }
    w.write_str("\n  ],\n")?;
    w.write_str("  \"regions\": [\n")?;
    write_regions(w, &dump.regions)?;
    w.write_str("  ],\n")?;
    w.write_str("  \"atc\": [\n")?;
    write_atc(w, &dump.atc)?;
    w.write_str("  ],\n")?;
    writeln!(w, "  \"atc_hits\": {},", dump.atc_hits)?;
    writeln!(w, "  \"atc_misses\": {}", dump.atc_misses)?;
    w.write_str("}\n")
}

fn write_ste_json<W: Write>(w: &mut W, slot: usize, s: &SteTableDump) -> fmt::Result {
    writeln!(w, "    {{")?;
    writeln!(w, "      \"slot\": {},", slot)?;
    writeln!(w, "      \"key\": \"{}\",", hex_u32(s.key))?;
    writeln!(w, "      \"state\": \"{}\",", s.state.as_str())?;
    writeln!(w, "      \"tenant\": {},", s.tenant.0)?;
    writeln!(w, "      \"config\": \"{}\",", s.config.as_str())?;
    writeln!(w, "      \"s1cdmax\": {},", s.s1cdmax)?;
    writeln!(w, "      \"distinct_ipa\": {},", s.distinct_ipa)?;
    writeln!(w, "      \"s2_vmid\": {},", s.s2_vmid)?;
    w.write_str("      \"cds\": [\n")?;
    let mut first_cd = true;
    for (cd_slot, cd) in s.cds.iter().enumerate() {
        let Some(c) = cd else {
            continue;
        };
        if !first_cd {
            w.write_str(",\n")?;
        }
        first_cd = false;
        write_cd_json(w, cd_slot, c)?;
    }
    w.write_str("\n      ],\n")?;
    w.write_str("      \"s2\": [\n")?;
    write_ptes(w, &s.s2, "        ")?;
    w.write_str("      ]\n")?;
    w.write_str("    }")
}

fn write_cd_json<W: Write>(w: &mut W, cd_slot: usize, c: &CdTableDump) -> fmt::Result {
    writeln!(w, "        {{")?;
    writeln!(w, "          \"cd_slot\": {},", cd_slot)?;
    writeln!(w, "          \"ssid\": {},", c.ssid)?;
    writeln!(w, "          \"valid\": {},", c.valid)?;
    writeln!(w, "          \"asid\": {},", c.asid)?;
    w.write_str("          \"s1\": [\n")?;
    write_ptes(w, &c.s1, "            ")?;
    w.write_str("          ]\n")?;
    w.write_str("        }")
}

fn write_ptes<W: Write>(w: &mut W, ptes: &[Option<SoftPte>], indent: &str) -> fmt::Result {
    let mut first = true;
    for p in ptes.iter().flatten() {
        if !first {
            w.write_str(",\n")?;
        }
        first = false;
        write!(
            w,
            "{indent}{{\"va\": \"{}\", \"out\": \"{}\", \"len\": \"{}\", \"writable\": {}}}",
            hex_u64(p.va),
            hex_u64(p.out),
            hex_u64(p.len),
            p.writable
        )?;
    }
    if !first {
        w.write_str("\n")?;
    }
    Ok(())
}

fn write_regions<W: Write>(w: &mut W, regions: &[Option<MappedRegion>; 16]) -> fmt::Result {
    let mut first = true;
    for r in regions.iter().flatten() {
        if !first {
            w.write_str(",\n")?;
        }
        first = false;
        write!(
            w,
            "    {{\"guest_pa\": \"{}\", \"iova\": \"{}\", \"len\": \"{}\", \"tenant\": {}, \"object\": {}, \"stream_id\": \"{}\", \"writable\": {}}}",
            hex_u64(r.guest_pa.0),
            hex_u64(r.iova.0),
            hex_u64(r.len),
            r.tenant.0,
            r.object,
            hex_u32(r.stream_id),
            r.writable
        )?;
    }
    if !first {
        w.write_str("\n")?;
    }
    Ok(())
}

fn write_atc<W: Write>(w: &mut W, atc: &[Option<AtcDumpLine>; 16]) -> fmt::Result {
    let mut first = true;
    for l in atc.iter().flatten() {
        if !first {
            w.write_str(",\n")?;
        }
        first = false;
        write!(
            w,
            "    {{\"sid\": \"{}\", \"iova\": \"{}\", \"pa\": \"{}\", \"len\": \"{}\"}}",
            hex_u32(l.sid),
            hex_u64(l.iova),
            hex_u64(l.pa),
            hex_u64(l.len)
        )?;
    }
    if !first {
        w.write_str("\n")?;
    }
    Ok(())
}

fn hex_u32(v: u32) -> Hex32 {
    Hex32(v)
}

fn hex_u64(v: u64) -> Hex64 {
    Hex64(v)
}

struct Hex32(u32);
struct Hex64(u64);

impl fmt::Display for Hex32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

impl fmt::Display for Hex64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iommu::{SteConfig, SOFT_SMMU_IOVA_BASE, SOFT_SMMU_IPA_BASE};

    const SCRIPT: &str = include_str!("../../docs/bringup/smmu_replay.jsonl");

    fn golden_path() -> &'static str {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/bringup/smmu_dump.golden.json"
        )
    }

    fn dump_string(dump: &SoftSmmuDump) -> String {
        let mut s = String::new();
        write_dump_json(dump, &mut s).unwrap();
        s
    }

    #[test]
    fn replay_jsonl_map_translate_abort_wrong_stream_ats() {
        let dump = replay_jsonl(SCRIPT).expect("bring-up replay");
        let got = dump_string(&dump);
        let path = golden_path();
        if std::env::var("GENERATE_SMMU_GOLDEN").is_ok() {
            std::fs::write(path, &got).expect("write golden dump");
        } else {
            let golden = std::fs::read_to_string(path).expect("golden dump");
            assert_eq!(
                got, golden,
                "software-table dump drifted from docs/bringup/smmu_dump.golden.json"
            );
        }
        let ste = dump.stes[0].as_ref().expect("one STE");
        assert_eq!(ste.state, StreamState::Bound);
        assert_eq!(ste.config, SteConfig::Nested);
        assert!(
            ste.distinct_ipa,
            "bind_nested so S1 IPA is not the guest PA"
        );
        let cd = ste.cds.iter().flatten().find(|c| c.ssid == KIT_CP_SSID);
        assert!(cd.is_some(), "Soft-CP SSID CD");
        let s1 = cd.unwrap().s1.iter().flatten().next().expect("S1 PTE");
        let s2 = ste.s2.iter().flatten().next().expect("S2 PTE");
        assert!(s1.va >= SOFT_SMMU_IOVA_BASE);
        assert!(s1.out >= SOFT_SMMU_IPA_BASE);
        assert_eq!(s2.out, KIT_GUEST_PA);
        assert_ne!(s1.va, s2.out, "IOVA is not identity");
        assert_ne!(s1.out, s2.out, "distinct IPA Stage-2");
        let iree_cd = ste.cds.iter().flatten().any(|c| c.ssid == KIT_IREE_SSID);
        assert!(iree_cd, "M1 IreeShapedCp SSID bound on the same STE");
        assert_eq!(
            dump.atc_len_for_test(),
            1,
            "ATC refill after Ats invalidate"
        );
    }

    impl SoftSmmuDump {
        fn atc_len_for_test(&self) -> usize {
            self.atc.iter().filter(|l| l.is_some()).count()
        }
    }

    #[test]
    fn script_names_the_kit_sequence() {
        assert!(SCRIPT.contains("\"op\": \"capture\""));
        assert!(SCRIPT.contains("\"op\": \"map\""));
        assert!(SCRIPT.contains("\"op\": \"translate\""));
        assert!(SCRIPT.contains("StreamAbort"));
        assert!(SCRIPT.contains("WrongStream"));
        assert!(SCRIPT.contains("\"op\": \"invalidate\""));
        assert!(SCRIPT.contains("\"cmd\": \"Ats\""));
        assert!(SCRIPT.contains("bind_nested"));
    }
}
