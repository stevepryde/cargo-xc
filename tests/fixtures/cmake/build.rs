fn main() {
    let destination = cmake::Config::new("native").build();
    println!("cargo:rustc-link-search=native={}", destination.join("lib").display());
    println!("cargo:rustc-link-lib=static=xc_cmake");
}
