//! Tiny TOML subset for the design-win worksheet. Not a general parser.

use std::collections::BTreeMap;

use crate::{CheckError, OpcodeMap, Worksheet};

#[derive(Clone, Debug)]
enum Val {
    Str(String),
    Int(u64),
    Strs(Vec<String>),
}

struct Doc {
    map: BTreeMap<String, Val>,
    arrays: BTreeMap<String, usize>,
    cur: String,
}

impl Doc {
    fn get_str(&self, k: &str) -> Result<&str, CheckError> {
        match self.map.get(k) {
            Some(Val::Str(s)) => Ok(s.as_str()),
            _ => Err(CheckError::Toml(format!("missing string {k}"))),
        }
    }

    fn get_u64(&self, k: &str) -> Result<u64, CheckError> {
        match self.map.get(k) {
            Some(Val::Int(v)) => Ok(*v),
            _ => Err(CheckError::Toml(format!("missing integer {k}"))),
        }
    }

    fn get_u32(&self, k: &str) -> Result<u32, CheckError> {
        let v = self.get_u64(k)?;
        u32::try_from(v).map_err(|_| CheckError::Toml(format!("{k} does not fit u32")))
    }

    fn get_u16(&self, k: &str) -> Result<u16, CheckError> {
        let v = self.get_u64(k)?;
        u16::try_from(v).map_err(|_| CheckError::Toml(format!("{k} does not fit u16")))
    }

    fn get_u8(&self, k: &str) -> Result<u8, CheckError> {
        let v = self.get_u64(k)?;
        u8::try_from(v).map_err(|_| CheckError::Toml(format!("{k} does not fit u8")))
    }

    fn get_strs(&self, k: &str) -> Result<&[String], CheckError> {
        match self.map.get(k) {
            Some(Val::Strs(v)) => Ok(v.as_slice()),
            _ => Err(CheckError::Toml(format!("missing array {k}"))),
        }
    }
}

/// Parse the worksheet TOML subset used by `sample.toml`.
pub fn parse_worksheet(src: &str) -> Result<Worksheet, CheckError> {
    let doc = parse_doc(src)?;
    let n = doc.arrays.get("opcode").copied().unwrap_or(0);
    if n == 0 {
        return Err(CheckError::Toml("no [[opcode]] rows".into()));
    }
    let mut opcodes = Vec::with_capacity(n);
    for i in 0..n {
        let p = format!("opcode.{i}");
        opcodes.push(OpcodeMap {
            their_name: doc.get_str(&format!("{p}.their_name"))?.to_string(),
            command_categories: doc.get_u16(&format!("{p}.iree_command_categories"))?,
            function: doc.get_u32(&format!("{p}.iree_function"))?,
            accel_op: doc.get_str(&format!("{p}.accel_op"))?.to_string(),
            dtype: doc.get_str(&format!("{p}.dtype"))?.to_string(),
            element_type: doc.get_u32(&format!("{p}.iree_element_type"))?,
            queue_affinity: doc.get_u32(&format!("{p}.queue_affinity"))?,
        });
    }
    Ok(Worksheet {
        party: doc.get_str("call.party")?.to_string(),
        backend_id: doc.get_u8("backend.id")?,
        backend_name: doc.get_str("backend.name")?.to_string(),
        opcodes,
        isa_blob_id: doc.get_u32("executable.isa_blob_id")?,
        sid_pool_size: doc.get_u64("sid.pool_size")? as usize,
        ssid: doc.get_u8("sid.ssid")?,
        chiplet: doc.get_u8("sid.chiplet")?,
        tile: doc.get_u16("sid.tile")?,
        submit_sid: doc.get_u32("sid.submit_sid")?,
        memory_spaces: doc
            .get_strs("memory.kinds")?
            .iter()
            .map(|s| s.clone())
            .collect(),
        queue_count: doc.get_u16("queues.count")?,
        event_scope: doc.get_str("event.scope")?.to_string(),
        chipsync_scopes: doc
            .get_strs("event.optional_chipsync_scopes")?
            .iter()
            .map(|s| s.clone())
            .collect(),
        frozen_packet_size: doc.get_u64("frozen.packet_size")? as usize,
        frozen_magic: doc.get_u32("frozen.magic")?,
    })
}

fn parse_doc(src: &str) -> Result<Doc, CheckError> {
    let mut doc = Doc {
        map: BTreeMap::new(),
        arrays: BTreeMap::new(),
        cur: String::new(),
    };
    for (lineno, raw) in src.lines().enumerate() {
        let stripped = strip_comment(raw);
        let line = stripped.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("[[") {
            let name = rest
                .strip_suffix("]]")
                .ok_or_else(|| CheckError::Toml(format!("line {}: bad array table", lineno + 1)))?
                .trim();
            let i = doc.arrays.entry(name.to_string()).or_insert(0);
            doc.cur = format!("{name}.{i}");
            *i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let name = rest
                .strip_suffix(']')
                .ok_or_else(|| CheckError::Toml(format!("line {}: bad table", lineno + 1)))?
                .trim();
            doc.cur = name.to_string();
            continue;
        }
        let (key, val) = split_kv(line).ok_or_else(|| {
            CheckError::Toml(format!("line {}: expected key = value", lineno + 1))
        })?;
        let full = if doc.cur.is_empty() {
            key.to_string()
        } else {
            format!("{}.{key}", doc.cur)
        };
        doc.map.insert(full, parse_val(val, lineno + 1)?);
    }
    Ok(doc)
}

fn strip_comment(line: &str) -> String {
    let mut out = String::new();
    let mut in_str = false;
    for c in line.chars() {
        if c == '"' {
            in_str = !in_str;
            out.push(c);
            continue;
        }
        if c == '#' && !in_str {
            break;
        }
        out.push(c);
    }
    out
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let eq = line.find('=')?;
    Some((line[..eq].trim(), line[eq + 1..].trim()))
}

fn parse_val(v: &str, line: usize) -> Result<Val, CheckError> {
    if let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        return Ok(Val::Str(inner.to_string()));
    }
    if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let mut strs = Vec::new();
        for part in inner.split(',') {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            let s = p
                .strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .ok_or_else(|| {
                    CheckError::Toml(format!("line {line}: array values must be strings"))
                })?;
            strs.push(s.to_string());
        }
        return Ok(Val::Strs(strs));
    }
    let int = if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16)
    } else {
        v.parse::<u64>()
    };
    int.map(Val::Int)
        .map_err(|_| CheckError::Toml(format!("line {line}: bad value {v}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_toml_has_six_opcodes() {
        let w = parse_worksheet(include_str!("../sample.toml")).unwrap();
        assert_eq!(w.opcodes.len(), 6);
        assert_eq!(w.opcodes[1].their_name, "gemm");
        assert_eq!(w.opcodes[3].accel_op, "Add");
        assert_eq!(w.opcodes[4].accel_op, "Relu");
        assert_eq!(w.opcodes[5].accel_op, "Mul");
    }
}
