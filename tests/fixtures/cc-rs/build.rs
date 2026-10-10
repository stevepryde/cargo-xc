fn main() {
    cc::Build::new().file("src/add.c").compile("xc_add");
    cc::Build::new()
        .cpp(true)
        .file("src/multiply.cpp")
        .compile("xc_multiply");
}
