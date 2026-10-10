use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/native.h");
    println!("cargo:rerun-if-changed=src/native.c");
    cc::Build::new().file("src/native.c").compile("xc_bindgen");
    let bindings = bindgen::Builder::default()
        .header("src/native.h")
        .allowlist_function("xc_bindgen_answer")
        .generate()
        .expect("bindgen must generate target declarations");
    bindings
        .write_to_file(PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("bindings.rs"))
        .expect("bindings must be writable");
}
