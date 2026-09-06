use std::env;
use std::fs;
use std::path::PathBuf;

fn embed(name: &str, filename: &str) {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_elf = manifest.join("../build").join(filename);
    println!("cargo:rerun-if-changed={}", repo_elf.display());

    let dest = if repo_elf.is_file() {
        repo_elf.canonicalize().unwrap()
    } else {
        let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join(filename);
        fs::write(&out, []).ok();
        out
    };
    println!("cargo:rustc-env={}={}", name, dest.display());
}

fn main() {
    embed("AETHER_INIT_ELF", "init.elf");
    embed("AETHER_PROBE_ELF", "probe.elf");
    embed("AETHER_INIT_ELF_RISCV", "init-riscv.elf");
    embed("AETHER_INIT_ELF_AARCH64", "init-aarch64.elf");
}
