//! `cargo run -p aether-partner-hello` — host leave-behind, no QEMU.

use aether_partner_hello::{format_packet, refuse_bad_executable, run_golden, PLUG_SCREEN};

fn main() {
    let report = run_golden().expect("partner-hello: golden IreeHalCmd path");
    println!("{}", format_packet(&report.cmd));
    println!(
        "[partner-hello] probe backend={} research_marker={:#06X} (not a silicon vendor)",
        report.backend, report.research_marker
    );
    println!("[partner-hello] golden matmul {:?} ok", report.result);

    refuse_bad_executable().expect("partner-hello: bad executable must be refused");
    println!("[partner-hello] bad executable 0xDEAD refused");
    print!("{PLUG_SCREEN}");
    println!(
        "[partner-hello] path B remains canonical (stock QEMU; this example does not rebuild QEMU)"
    );
    println!("[partner-hello] ok");
}
