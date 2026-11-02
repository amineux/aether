//! Layout representability for the frozen CP and IREE-shaped packets.

use aether_core::accel::{AccelJobDesc, AccelOp};
use aether_hal::HalError;

/// Neither packet encodes row strides. Refuse a host template whose active
/// rows would access a different layout than the packed image authorizes.
/// Single-row strides and unused operands do not affect execution.
pub(crate) fn check(job: &AccelJobDesc) -> Result<(), HalError> {
    if job.op == AccelOp::Nop {
        return Ok(());
    }
    let ew = job.op.is_elementwise();
    let a_cols = if ew { job.n } else { job.k };
    let b_rows = if ew { job.m } else { job.k };
    if (job.m > 1 && job.a_stride != a_cols)
        || (job.op != AccelOp::Relu && b_rows > 1 && job.b_stride != job.n)
        || (job.m > 1 && job.c_stride != job.n)
    {
        return Err(HalError::Unsupported);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::types::PhysAddr;

    #[test]
    fn rectangular_layout_uses_operation_specific_rows_and_columns() {
        let mut job = AccelJobDesc::matmul_i32(2, 3, 4, PhysAddr(0), PhysAddr(32), PhysAddr(80), 1);
        assert_eq!(check(&job), Ok(()));
        job.a_stride = job.n;
        assert_eq!(check(&job), Err(HalError::Unsupported));
        job.op = AccelOp::Add;
        assert_eq!(check(&job), Ok(())); // A now has N columns
        job.k = 1;
        job.b_stride = 0;
        assert_eq!(check(&job), Err(HalError::Unsupported)); // B has M rows, not K
        job.op = AccelOp::Relu;
        assert_eq!(check(&job), Ok(()));
    }

    #[test]
    fn only_active_row_strides_must_be_packed() {
        let mut job = AccelJobDesc::matmul_i32(1, 2, 1, PhysAddr(0), PhysAddr(16), PhysAddr(32), 1);
        job.a_stride = u32::MAX;
        job.b_stride = u32::MAX;
        job.c_stride = u32::MAX;
        assert_eq!(check(&job), Ok(())); // all three operands have one row
        job.k = 2;
        assert_eq!(check(&job), Err(HalError::Unsupported)); // B now has two rows
        job.op = AccelOp::Relu;
        assert_eq!(check(&job), Ok(())); // B is unused
        job.m = 2;
        job.a_stride = job.n;
        job.c_stride = job.n;
        assert_eq!(check(&job), Ok(()));
        job.op = AccelOp::Nop;
        job.a_stride = 0;
        job.c_stride = 0;
        assert_eq!(check(&job), Ok(()));
    }
}
