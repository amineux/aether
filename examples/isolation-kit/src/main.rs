//! `aether-isolation-kit` — run the pre-silicon tenant-isolation conformance
//! matrix over the shipped adapters and print, for each backend, the matrix
//! and a grep-able one-line summary.
//!
//! Exit code is 0 on a successful run: the weak sample backend is *expected*
//! to be non-conformant (that is the point), so the per-backend verdict lives
//! in the summary lines, not in the process exit code. `make isolation-matrix`
//! greps those lines.

use aether_isolation_kit::{extra_backend, shipped_backends, Matrix};

fn main() {
    // Optional extra backend (`make isolation-matrix BACKEND=template`). With
    // no argument the output is exactly the shipped two-backend run.
    let mut backends = shipped_backends();
    let extra = std::env::args().nth(1).filter(|a| !a.is_empty());
    if let Some(name) = &extra {
        match extra_backend(name) {
            Some(b) => backends.push(b),
            None => {
                eprintln!("aether-isolation-kit: unknown backend {name:?} (known: template)");
                std::process::exit(2);
            }
        }
    }

    println!("Aether pre-silicon tenant-isolation conformance kit (host-only).");
    println!("A backend implements the IsolationBackend trait; the kit runs the existing");
    println!("named attack classes and reports refused / ACCEPTED / n/a per class.");
    println!("Not certification, not a partner result, no hardware or performance claims.");
    println!();

    for backend in backends {
        let matrix = Matrix::run(backend.as_ref());
        print!("{}", matrix.table());
        println!("{}", matrix.summary_line());
        println!();
    }

    println!("[isolation-kit] reference=aether-soft is the conformance baseline;");
    println!("[isolation-kit] weak-sample-example-only is a teaching stub whose accepts");
    println!("[isolation-kit] demonstrate the kit can fail. It is not a real backend.");
    if extra.as_deref() == Some("template") {
        println!("[isolation-kit] template-unimplemented is the bring-your-own-backend starting");
        println!("[isolation-kit] point (examples/isolation-kit/template); every class is n/a until filled in.");
    }
}
