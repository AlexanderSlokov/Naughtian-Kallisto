fn main() {
    cxx_build::bridge("src/lib.rs")
        .std("c++20")
        .compile("ffi_bridge");

    println!("cargo:rerun-if-changed=src/lib.rs");
}
