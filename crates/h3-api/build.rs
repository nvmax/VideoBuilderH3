//! The app's screens are embedded in the program (`src/embedded.rs`). A change to them must build it again.

fn main() {
    println!("cargo:rerun-if-changed=../../app/web");
}
