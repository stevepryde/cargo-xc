unsafe extern "C" {
    fn xc_cmake_answer() -> i32;
}

fn main() {
    // SAFETY: the CMake fixture builds this exact no-argument function.
    assert_eq!(unsafe { xc_cmake_answer() }, 42);
    println!("cmake passed");
}
