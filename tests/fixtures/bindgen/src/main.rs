include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

fn main() {
    // SAFETY: bindgen generated this declaration from the bundled header.
    assert_eq!(unsafe { xc_bindgen_answer() }, 42);
    println!("bindgen passed");
}
