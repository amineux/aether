use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_elf = manifest.join("../build/init.elf");
    println!("cargo:rerun-if-changed={}", repo_elf.display());

    let dest = if repo_elf.is_file() {
        repo_elf.canonicalize().unwrap()
    } else {
        let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("init.elf");
        fs::write(&out, []).ok();
        out
    };
    println!("cargo:rustc-env=AETHER_INIT_ELF={}", dest.display());
}
