//! `aether-isolation-kit` — run the pre-silicon tenant-isolation conformance
//! matrix over the shipped adapters and print, for each backend, the matrix
//! and a grep-able one-line summary.
//!
//! Exit code is 0 on a successful run: the weak sample backend is *expected*
//! to be non-conformant (that is the point), so the per-backend verdict lives
//! in the summary lines, not in the process exit code. `make isolation-matrix`
//! greps those lines.

use aether_isolation_kit::{shipped_backends, Matrix};

fn main() {
    println!("Aether pre-silicon tenant-isolation conformance kit (host-only).");
    println!("A backend implements the IsolationBackend trait; the kit runs the existing");
    println!("named attack classes and reports refused / ACCEPTED / n/a per class.");
    println!("Not certification, not a partner result, no hardware or performance claims.");
    println!();

    for backend in shipped_backends() {
        let matrix = Matrix::run(backend.as_ref());
        print!("{}", matrix.table());
        println!("{}", matrix.summary_line());
        println!();
    }

    println!("[isolation-kit] reference=aether-soft is the conformance baseline;");
    println!("[isolation-kit] weak-sample-example-only is a teaching stub whose accepts");
    println!("[isolation-kit] demonstrate the kit can fail. It is not a real backend.");
}
