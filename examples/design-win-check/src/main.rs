//! Load a filled design-win worksheet and print admit / refuse.
//!
//! Default: the bundled `sample.toml`. Pass a path to check a copy
//! filled on a call.

use std::env;
use std::fs;
use std::process::ExitCode;

use aether_design_win_check::{check_worksheet, probe_iree_shaped_backend, Worksheet, SAMPLE_TOML};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let (label, src) = if let Some(path) = args.get(1) {
        match fs::read_to_string(path) {
            Ok(s) => (path.clone(), s),
            Err(e) => {
                eprintln!("design-win-check: failed to read {path}: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        ("sample.toml".into(), SAMPLE_TOML.to_string())
    };

    let w = match Worksheet::from_toml(&src) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("design-win-check: {label}: {e}");
            return ExitCode::from(1);
        }
    };

    if let Err(e) = check_worksheet(&w).and_then(|_| probe_iree_shaped_backend()) {
        eprintln!("design-win-check: REFUSE {label}: {e}");
        return ExitCode::from(1);
    }

    println!("design-win-check: ADMIT {label}");
    println!("  party            {}", w.party);
    println!("  backend          {} ({})", w.backend_id, w.backend_name);
    println!("  executable       {:#010x}", w.isa_blob_id);
    println!(
        "  submit SID       {:#010x} (ssid={} pool={})",
        w.submit_sid, w.ssid, w.sid_pool_size
    );
    println!("  queues           {}", w.queue_count);
    println!("  memory           {}", w.memory_spaces.join(", "));
    println!("  event            {}", w.event_scope);
    println!("  opcodes:");
    for op in &w.opcodes {
        println!(
            "    {:<16} categories={:#06x} function={} → {} ({})",
            op.their_name, op.command_categories, op.function, op.accel_op, op.dtype
        );
    }
    println!("  frozen IreeHalCmd offsets held (96-byte v1; not relocated)");
    ExitCode::SUCCESS
}
