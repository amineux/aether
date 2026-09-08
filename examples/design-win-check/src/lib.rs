//! Host fixture for a filled [`docs/DESIGN_WIN.md`] worksheet.
//!
//! Loads the sample TOML (or a caller-supplied copy) and admits it only
//! when the mapping is a legal v1 [`IreeHalCmd`]: known
//! [`IREE_REF_EXECUTABLE`], nonzero Soft-SMMU SID (`ssid = 2`), and
//! `command_categories` ∈ {0, DISPATCH}. Unknown executable id, SID 0,
//! and TRANSFER-only packets are refused — the worksheet is executable,
//! not a PDF.
//!
//! Not a signed vendor, not an IREE runtime, not a PJRT plugin.
//! Frozen `IreeHalCmd` offsets are asserted, never relocated.
//!
//! [`docs/DESIGN_WIN.md`]: https://github.com/amineux/aether/blob/main/docs/DESIGN_WIN.md

#![deny(unsafe_code)]

use aether_core::accel::{AccelOp, DType};
use aether_core::iommu::{StreamId, DEFAULT_STREAM, SID_BUDGET_PER_TENANT};
use aether_core::space::MemorySpace;
use aether_core::types::{ChipletId, TileId};
use aether_drivers::ireecp::{
    categories_from_op, element_type_from_dtype, function_from_op, op_from_hal, IreeHalCmd,
    HAL_FN_FUSED, HAL_FN_MATMUL, IREE_HAL_CMD_SIZE, IREE_HAL_COMMAND_CATEGORY_DISPATCH,
    IREE_HAL_COMMAND_CATEGORY_TRANSFER, IREE_HAL_ELEMENT_TYPE_FLOAT_16,
    IREE_HAL_ELEMENT_TYPE_FLOAT_32, IREE_HAL_ELEMENT_TYPE_INT_32, IREE_HAL_PKT_MAGIC,
    IREE_REF_EXECUTABLE, IREE_SSID,
};
use aether_hal::{AccelDevice, HalError, ACCEL_BACKEND_IREE_SHAPED};

mod parse;

pub use parse::parse_worksheet;

/// Bundled sample filled on the research mapping (not a vendor ISA).
pub const SAMPLE_TOML: &str = include_str!("../sample.toml");

/// IREE HAL research stand-in (public nouns already frozen on IreeHalCmd).
/// Not a partner. Path: `docs/design-win/iree-hal-standin.toml`.
pub const STANDIN_TOML: &str = include_str!("../../../docs/design-win/iree-hal-standin.toml");

/// Frozen `IreeHalCmd` v1 field offsets from `docs/ACCEL.md`. Do not change.
pub const FROZEN_OFFSETS: &[(usize, &'static str)] = &[
    (0x00, "magic"),
    (0x04, "command_categories"),
    (0x06, "binding_count"),
    (0x08, "executable"),
    (0x0C, "function"),
    (0x10, "workgroup_count_x"),
    (0x14, "workgroup_count_y"),
    (0x18, "workgroup_count_z"),
    (0x1C, "element_type"),
    (0x20, "queue_affinity"),
    (0x24, "stream_id"),
    (0x28, "binding0_offset"),
    (0x30, "binding1_offset"),
    (0x38, "binding2_offset"),
    (0x40, "binding3_offset"),
    (0x48, "binding0_length"),
    (0x4C, "binding1_length"),
    (0x50, "binding2_length"),
    (0x54, "binding3_length"),
    (0x58, "signal_payload"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpcodeMap {
    pub their_name: String,
    pub command_categories: u16,
    pub function: u32,
    pub accel_op: String,
    pub dtype: String,
    pub element_type: u32,
    pub queue_affinity: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worksheet {
    pub party: String,
    pub backend_id: u8,
    pub backend_name: String,
    pub opcodes: Vec<OpcodeMap>,
    pub isa_blob_id: u32,
    pub sid_pool_size: usize,
    pub ssid: u8,
    pub chiplet: u8,
    pub tile: u16,
    pub submit_sid: u32,
    pub memory_spaces: Vec<String>,
    pub queue_count: u16,
    pub event_scope: String,
    pub chipsync_scopes: Vec<String>,
    pub frozen_packet_size: usize,
    pub frozen_magic: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckError {
    UnknownExecutable(u32),
    SidZero,
    TransferOnly { their_name: String },
    Hal(HalError),
    OpcodeMismatch { their_name: String, detail: String },
    UnknownMemorySpace(String),
    SidPool,
    QueueCount { got: u16 },
    Backend { got: u8 },
    FrozenMismatch(String),
    EventScope(String),
    Toml(String),
}

impl core::fmt::Display for CheckError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownExecutable(id) => {
                write!(
                    f,
                    "unknown executable id {id:#010x} (v1 admits IREE_REF_EXECUTABLE {IREE_REF_EXECUTABLE:#010x} only)"
                )
            }
            Self::SidZero => {
                write!(
                    f,
                    "SID 0 / DEFAULT_STREAM is reserved (SoftNPU / PASID-0 analogue); IreeShapedCp submit SID must be nonzero with ssid={IREE_SSID}"
                )
            }
            Self::TransferOnly { their_name } => {
                write!(
                    f,
                    "opcode {their_name:?} is TRANSFER-only; v1 IreeHalCmd does not define that packet"
                )
            }
            Self::Hal(e) => write!(f, "HAL refuse {e:?}"),
            Self::OpcodeMismatch { their_name, detail } => {
                write!(f, "opcode {their_name:?}: {detail}")
            }
            Self::UnknownMemorySpace(s) => write!(f, "unknown MemorySpace {s:?}"),
            Self::SidPool => write!(
                f,
                "SID pool must be 1..=SID_BUDGET_PER_TENANT ({SID_BUDGET_PER_TENANT})"
            ),
            Self::QueueCount { got } => {
                write!(f, "IreeShapedCp v1 is a single mailbox; queue_count={got}")
            }
            Self::Backend { got } => {
                write!(
                    f,
                    "backend id {got} is not IreeShapedCp ({ACCEL_BACKEND_IREE_SHAPED})"
                )
            }
            Self::FrozenMismatch(s) => write!(f, "frozen IreeHalCmd mismatch: {s}"),
            Self::EventScope(s) => write!(f, "event scope {s:?} is not a partition timeline"),
            Self::Toml(s) => write!(f, "worksheet TOML: {s}"),
        }
    }
}

impl Worksheet {
    pub fn from_toml(src: &str) -> Result<Self, CheckError> {
        parse_worksheet(src)
    }

    pub fn sample() -> Self {
        parse_worksheet(SAMPLE_TOML).expect("bundled sample.toml is well-formed")
    }

    /// Packed StreamId the SID table claims. Must match `submit_sid`.
    pub fn expected_sid(&self) -> StreamId {
        StreamId::accel(ChipletId(self.chiplet), TileId(self.tile), self.ssid)
    }
}

/// Admit a filled worksheet against the frozen IREE HAL packet.
pub fn check_worksheet(w: &Worksheet) -> Result<(), CheckError> {
    if w.backend_id != ACCEL_BACKEND_IREE_SHAPED {
        return Err(CheckError::Backend { got: w.backend_id });
    }
    if w.backend_name != "iree-shaped-cp" {
        return Err(CheckError::FrozenMismatch(format!(
            "backend name {:?} (expected iree-shaped-cp)",
            w.backend_name
        )));
    }
    if w.isa_blob_id != IREE_REF_EXECUTABLE {
        return Err(CheckError::UnknownExecutable(w.isa_blob_id));
    }
    if w.submit_sid == DEFAULT_STREAM || w.submit_sid == 0 || w.ssid == 0 {
        return Err(CheckError::SidZero);
    }
    if w.ssid != IREE_SSID {
        return Err(CheckError::FrozenMismatch(format!(
            "ssid {} is not IREE_SSID {IREE_SSID}",
            w.ssid
        )));
    }
    if w.expected_sid().raw() != w.submit_sid {
        return Err(CheckError::FrozenMismatch(format!(
            "submit_sid {:#010x} != packed StreamId {:#010x}",
            w.submit_sid,
            w.expected_sid().raw()
        )));
    }
    if w.sid_pool_size == 0 || w.sid_pool_size > SID_BUDGET_PER_TENANT {
        return Err(CheckError::SidPool);
    }
    if w.queue_count != 1 {
        return Err(CheckError::QueueCount { got: w.queue_count });
    }
    if w.frozen_packet_size != IREE_HAL_CMD_SIZE {
        return Err(CheckError::FrozenMismatch(format!(
            "packet_size {} (frozen {IREE_HAL_CMD_SIZE})",
            w.frozen_packet_size
        )));
    }
    if w.frozen_magic != IREE_HAL_PKT_MAGIC {
        return Err(CheckError::FrozenMismatch(format!(
            "magic {:#010x} (frozen {IREE_HAL_PKT_MAGIC:#010x})",
            w.frozen_magic
        )));
    }
    if w.event_scope != "partition-timeline" {
        return Err(CheckError::EventScope(w.event_scope.clone()));
    }
    for kind in &w.memory_spaces {
        parse_space(kind)?;
    }
    if w.opcodes.is_empty() {
        return Err(CheckError::Toml("opcode map is empty".into()));
    }
    for op in &w.opcodes {
        check_opcode(w, op)?;
    }
    check_frozen_offsets()?;
    Ok(())
}

fn parse_space(name: &str) -> Result<MemorySpace, CheckError> {
    match name {
        "HOST" => Ok(MemorySpace::Host),
        "DEVICE_HBM" => Ok(MemorySpace::DeviceHbm),
        "TILE_SRAM" => Ok(MemorySpace::TileSram),
        "CXL_REGION" => Ok(MemorySpace::CxlRegion),
        "SCRATCH" => Ok(MemorySpace::Scratch),
        "STREAMING" => Ok(MemorySpace::Streaming),
        other => Err(CheckError::UnknownMemorySpace(other.to_string())),
    }
}

fn parse_accel_op(name: &str) -> Result<AccelOp, CheckError> {
    match name {
        "Nop" => Ok(AccelOp::Nop),
        "MatMul" => Ok(AccelOp::MatMul),
        "Wave" => Ok(AccelOp::Wave),
        other => Err(CheckError::OpcodeMismatch {
            their_name: other.to_string(),
            detail: "AccelOp must be Nop, MatMul, or Wave".into(),
        }),
    }
}

fn parse_dtype(name: &str) -> Result<DType, CheckError> {
    match name {
        "I32" => Ok(DType::I32),
        "F16" => Ok(DType::F16),
        "F32" => Ok(DType::F32),
        other => Err(CheckError::OpcodeMismatch {
            their_name: other.to_string(),
            detail: format!("unknown DType {other}"),
        }),
    }
}

fn check_opcode(w: &Worksheet, op: &OpcodeMap) -> Result<(), CheckError> {
    if op.command_categories == IREE_HAL_COMMAND_CATEGORY_TRANSFER {
        return Err(CheckError::TransferOnly {
            their_name: op.their_name.clone(),
        });
    }
    let accel = parse_accel_op(&op.accel_op)?;
    let dtype = parse_dtype(&op.dtype)?;
    match op_from_hal(op.command_categories, op.function) {
        Ok(decoded) if decoded == accel => {}
        Ok(decoded) => {
            return Err(CheckError::OpcodeMismatch {
                their_name: op.their_name.clone(),
                detail: format!("HAL decode {decoded:?} != filled AccelOp {accel:?}"),
            })
        }
        Err(HalError::Fault)
            if op.command_categories & IREE_HAL_COMMAND_CATEGORY_TRANSFER != 0
                && op.command_categories & IREE_HAL_COMMAND_CATEGORY_DISPATCH == 0 =>
        {
            return Err(CheckError::TransferOnly {
                their_name: op.their_name.clone(),
            });
        }
        Err(e) => return Err(CheckError::Hal(e)),
    }
    if categories_from_op(accel) != op.command_categories {
        return Err(CheckError::OpcodeMismatch {
            their_name: op.their_name.clone(),
            detail: "command_categories does not match AccelOp pack".into(),
        });
    }
    let expected_fn = match accel {
        AccelOp::Nop => 0,
        AccelOp::MatMul => HAL_FN_MATMUL,
        AccelOp::Wave => HAL_FN_FUSED,
    };
    // Nop pack writes function = 0; decode ignores it. Filled function must
    // still be the pack value so a shim cannot branch on function first.
    if function_from_op(accel) != op.function || op.function != expected_fn {
        return Err(CheckError::OpcodeMismatch {
            their_name: op.their_name.clone(),
            detail: "function ordinal does not match AccelOp pack".into(),
        });
    }
    if element_type_from_dtype(dtype) != op.element_type {
        return Err(CheckError::OpcodeMismatch {
            their_name: op.their_name.clone(),
            detail: "element_type does not match DType".into(),
        });
    }
    match op.element_type {
        IREE_HAL_ELEMENT_TYPE_INT_32
        | IREE_HAL_ELEMENT_TYPE_FLOAT_16
        | IREE_HAL_ELEMENT_TYPE_FLOAT_32 => {}
        other => {
            return Err(CheckError::OpcodeMismatch {
                their_name: op.their_name.clone(),
                detail: format!("unrecognized iree_hal_element_type_t {other:#x}"),
            })
        }
    }
    let cmd = packet_from_opcode(w, op);
    cmd.check_v1().map_err(|e| match e {
        HalError::Unsupported => CheckError::UnknownExecutable(cmd.executable),
        HalError::Fault
            if cmd.command_categories == IREE_HAL_COMMAND_CATEGORY_TRANSFER
                || (cmd.command_categories & IREE_HAL_COMMAND_CATEGORY_TRANSFER != 0
                    && cmd.command_categories & IREE_HAL_COMMAND_CATEGORY_DISPATCH == 0) =>
        {
            CheckError::TransferOnly {
                their_name: op.their_name.clone(),
            }
        }
        other => CheckError::Hal(other),
    })?;
    if cmd.stream_id == 0 {
        return Err(CheckError::SidZero);
    }
    Ok(())
}

fn packet_from_opcode(w: &Worksheet, op: &OpcodeMap) -> IreeHalCmd {
    IreeHalCmd {
        magic: IREE_HAL_PKT_MAGIC,
        command_categories: op.command_categories,
        binding_count: if op.command_categories == 0 { 0 } else { 3 },
        executable: w.isa_blob_id,
        function: op.function,
        workgroup_count_x: 1,
        workgroup_count_y: 1,
        workgroup_count_z: 1,
        element_type: op.element_type,
        queue_affinity: op.queue_affinity,
        stream_id: w.submit_sid,
        binding0_offset: 0,
        binding1_offset: 0,
        binding2_offset: 0,
        binding3_offset: 0,
        binding0_length: 0,
        binding1_length: 0,
        binding2_length: 0,
        binding3_length: 0,
        signal_payload: 1,
    }
}

/// Re-pack sentinels and assert each frozen field still sits at the
/// documented offset. Does not relocate anything in `ireecp.rs`.
pub fn check_frozen_offsets() -> Result<(), CheckError> {
    if core::mem::size_of::<IreeHalCmd>() != IREE_HAL_CMD_SIZE {
        return Err(CheckError::FrozenMismatch(format!(
            "size_of::<IreeHalCmd>() is {} not {IREE_HAL_CMD_SIZE}",
            core::mem::size_of::<IreeHalCmd>()
        )));
    }
    let cmd = IreeHalCmd {
        magic: 0x1111_1111,
        command_categories: 0x2222,
        binding_count: 0x3333,
        executable: 0x4444_4444,
        function: 0x5555_5555,
        workgroup_count_x: 0x6666_6666,
        workgroup_count_y: 0x7777_7777,
        workgroup_count_z: 0x8888_8888,
        element_type: 0x9999_9999,
        queue_affinity: 0xAAAA_AAAA,
        stream_id: 0xBBBB_BBBB,
        binding0_offset: 0x0101_0101_0101_0101,
        binding1_offset: 0x0202_0202_0202_0202,
        binding2_offset: 0x0303_0303_0303_0303,
        binding3_offset: 0x0404_0404_0404_0404,
        binding0_length: 0xC0C0_C0C0,
        binding1_length: 0xD1D1_D1D1,
        binding2_length: 0xE2E2_E2E2,
        binding3_length: 0xF3F3_F3F3,
        signal_payload: 0x0505_0505_0505_0505,
    };
    let b = cmd.to_le_bytes();
    if b.len() != IREE_HAL_CMD_SIZE {
        return Err(CheckError::FrozenMismatch("to_le_bytes length".into()));
    }
    let u16_at = |off: usize| u16::from_le_bytes(b[off..off + 2].try_into().unwrap());
    let u32_at = |off: usize| u32::from_le_bytes(b[off..off + 4].try_into().unwrap());
    let u64_at = |off: usize| u64::from_le_bytes(b[off..off + 8].try_into().unwrap());
    let checks: &[(usize, &str, u64, u8)] = &[
        (0x00, "magic", cmd.magic as u64, 4),
        (0x04, "command_categories", cmd.command_categories as u64, 2),
        (0x06, "binding_count", cmd.binding_count as u64, 2),
        (0x08, "executable", cmd.executable as u64, 4),
        (0x0C, "function", cmd.function as u64, 4),
        (0x10, "workgroup_count_x", cmd.workgroup_count_x as u64, 4),
        (0x14, "workgroup_count_y", cmd.workgroup_count_y as u64, 4),
        (0x18, "workgroup_count_z", cmd.workgroup_count_z as u64, 4),
        (0x1C, "element_type", cmd.element_type as u64, 4),
        (0x20, "queue_affinity", cmd.queue_affinity as u64, 4),
        (0x24, "stream_id", cmd.stream_id as u64, 4),
        (0x28, "binding0_offset", cmd.binding0_offset, 8),
        (0x30, "binding1_offset", cmd.binding1_offset, 8),
        (0x38, "binding2_offset", cmd.binding2_offset, 8),
        (0x40, "binding3_offset", cmd.binding3_offset, 8),
        (0x48, "binding0_length", cmd.binding0_length as u64, 4),
        (0x4C, "binding1_length", cmd.binding1_length as u64, 4),
        (0x50, "binding2_length", cmd.binding2_length as u64, 4),
        (0x54, "binding3_length", cmd.binding3_length as u64, 4),
        (0x58, "signal_payload", cmd.signal_payload, 8),
    ];
    if checks.len() != FROZEN_OFFSETS.len() {
        return Err(CheckError::FrozenMismatch(
            "offset table length drifted from ACCEL.md list".into(),
        ));
    }
    for ((off, name, expect, width), (listed_off, listed_name)) in
        checks.iter().zip(FROZEN_OFFSETS.iter())
    {
        if off != listed_off || name != listed_name {
            return Err(CheckError::FrozenMismatch(format!(
                "offset table {name} @{off:#x} != listed {listed_name} @{listed_off:#x}"
            )));
        }
        let got = match width {
            2 => u16_at(*off) as u64,
            4 => u32_at(*off) as u64,
            8 => u64_at(*off),
            _ => unreachable!(),
        };
        if got != *expect {
            return Err(CheckError::FrozenMismatch(format!(
                "{name} @{off:#x} packed {got:#x} expected {expect:#x}"
            )));
        }
    }
    Ok(())
}

/// Probe the in-tree IreeShapedCp so the worksheet's backend id matches
/// a live `AccelDevice`, not a slide.
pub fn probe_iree_shaped_backend() -> Result<(), CheckError> {
    let mut dev = aether_drivers::IreeShapedCp::new(aether_drivers::IdentityDma);
    let info = AccelDevice::probe(&mut dev).map_err(CheckError::Hal)?;
    if info.backend != ACCEL_BACKEND_IREE_SHAPED {
        return Err(CheckError::Backend { got: info.backend });
    }
    if info.n_queues != 1 {
        return Err(CheckError::QueueCount { got: info.n_queues });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_worksheet_is_admitted() {
        let w = Worksheet::sample();
        check_worksheet(&w).unwrap();
        probe_iree_shaped_backend().unwrap();
        assert_eq!(w.party, "research-runtime-example");
        assert_eq!(w.opcodes[0].accel_op, "Nop");
        assert_eq!(w.opcodes[1].accel_op, "MatMul");
        assert_eq!(w.opcodes[2].accel_op, "Wave");
        assert_ne!(w.submit_sid, DEFAULT_STREAM);
    }

    #[test]
    fn iree_hal_standin_worksheet_is_admitted() {
        let w = Worksheet::from_toml(STANDIN_TOML).expect("stand-in TOML is well-formed");
        check_worksheet(&w).unwrap();
        probe_iree_shaped_backend().unwrap();
        assert_eq!(w.party, "iree-hal-research-standin");
        assert_eq!(w.frozen_magic, IREE_HAL_PKT_MAGIC);
        assert_eq!(w.isa_blob_id, IREE_REF_EXECUTABLE);
        assert_eq!(w.opcodes[0].their_name, "iree_hal_command_buffer");
        assert_eq!(w.opcodes[0].command_categories, 0);
        assert_eq!(w.opcodes[1].their_name, "iree_hal_command_buffer_dispatch");
        assert_eq!(
            w.opcodes[1].command_categories,
            IREE_HAL_COMMAND_CATEGORY_DISPATCH
        );
        assert_eq!(w.opcodes[2].their_name, "iree_hal_device_queue_dispatch");
        assert_eq!(w.ssid, IREE_SSID);
        assert_eq!(w.sid_pool_size, SID_BUDGET_PER_TENANT);
        assert_ne!(w.submit_sid, DEFAULT_STREAM);
        assert!(w
            .opcodes
            .iter()
            .all(|op| op.command_categories != IREE_HAL_COMMAND_CATEGORY_TRANSFER));
    }

    #[test]
    fn unknown_executable_is_refused() {
        let mut w = Worksheet::sample();
        w.isa_blob_id = 0xDEAD_BEEF;
        assert_eq!(
            check_worksheet(&w),
            Err(CheckError::UnknownExecutable(0xDEAD_BEEF))
        );
        let mut cmd = packet_from_opcode(&w, &w.opcodes[1]);
        cmd.executable = 0xDEAD_BEEF;
        assert_eq!(cmd.check_v1(), Err(HalError::Unsupported));
    }

    #[test]
    fn sid_zero_is_refused() {
        let mut w = Worksheet::sample();
        w.submit_sid = 0;
        w.ssid = 0;
        w.chiplet = 0;
        w.tile = 0;
        assert_eq!(check_worksheet(&w), Err(CheckError::SidZero));
        w.ssid = IREE_SSID;
        w.submit_sid = 0;
        assert_eq!(check_worksheet(&w), Err(CheckError::SidZero));
        assert_eq!(DEFAULT_STREAM, 0);
    }

    #[test]
    fn transfer_only_packet_is_refused() {
        let mut w = Worksheet::sample();
        w.opcodes[1].command_categories = IREE_HAL_COMMAND_CATEGORY_TRANSFER;
        w.opcodes[1].their_name = "dma_copy".into();
        assert_eq!(
            check_worksheet(&w),
            Err(CheckError::TransferOnly {
                their_name: "dma_copy".into()
            })
        );
        let mut cmd = packet_from_opcode(&w, &w.opcodes[1]);
        cmd.command_categories = IREE_HAL_COMMAND_CATEGORY_TRANSFER;
        cmd.executable = IREE_REF_EXECUTABLE;
        assert_eq!(cmd.check_v1(), Err(HalError::Fault));
    }

    #[test]
    fn frozen_iree_hal_cmd_offsets_hold() {
        check_frozen_offsets().unwrap();
        assert_eq!(FROZEN_OFFSETS[0], (0x00, "magic"));
        assert_eq!(FROZEN_OFFSETS[10], (0x24, "stream_id"));
        assert_eq!(FROZEN_OFFSETS[19], (0x58, "signal_payload"));
    }

    #[test]
    fn unknown_memory_space_is_refused() {
        let mut w = Worksheet::sample();
        w.memory_spaces.push("UNIFIED".into());
        assert_eq!(
            check_worksheet(&w),
            Err(CheckError::UnknownMemorySpace("UNIFIED".into()))
        );
    }
}
