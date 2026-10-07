//! Two-tenant inference isolation demo (host). See `scenario.rs`.
//!
//! Run: `make two-tenant-infer` or
//! `cargo run -p aether-redteam --example two_tenant_infer`.

mod scenario;

fn main() {
    let s = scenario::summarize(scenario::Config::with_attacker());
    for l in &s.run.log {
        println!("{l}");
    }
    println!("{}", scenario::SCOPE_LINE);
    println!("{}", s.line());
    if !s.all_ok() {
        std::process::exit(1);
    }
}
