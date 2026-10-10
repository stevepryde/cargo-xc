unsafe extern "C" {
    fn xc_add(left: i32, right: i32) -> i32;
    fn xc_multiply(left: i32, right: i32) -> i32;
}

fn main() {
    // SAFETY: both functions are built from the fixture sources with matching signatures.
    unsafe {
        assert_eq!(xc_add(2, 3), 5);
        assert_eq!(xc_multiply(4, 5), 20);
    }
    println!("native dependencies passed");
}
