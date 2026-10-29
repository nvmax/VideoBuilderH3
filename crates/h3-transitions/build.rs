// The effects and filters are embedded with `include_dir!`, which does not notice a changed file by itself: rebuild when the shader folders change.
fn main() {
    println!("cargo:rerun-if-changed=looks");
    println!("cargo:rerun-if-changed=transitions");
}
