//! Partner one-command: `make diligence-demo` / `cargo diligence-demo`.

fn main() {
    let mut out = String::new();
    match aether_diligence_demo::run_diligence_demo(&mut out) {
        Ok(()) => print!("{out}"),
        Err(e) => {
            print!("{out}");
            eprintln!("diligence-demo FAIL: {e}");
            std::process::exit(1);
        }
    }
}
