fn main() {
    cxx_build::bridge("src/lib.rs")
        .file("src/bridge.cpp") // We can add C++ files here if needed, but for now just the bridge
        .std("c++20")
        .compile("ffi_bridge");

    println!("cargo:rerun-if-changed=src/lib.rs");
}
