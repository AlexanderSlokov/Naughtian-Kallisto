#include <iostream>
#include "rust_integrates/ffi_bridge/src/lib.rs.h"

int main() {
    std::cout << "[C++] Calling Rust FFI..." << std::endl;
    
    // Call Rust function
    rust::String version = kallisto::rust::get_rust_version();
    std::cout << "[C++] Rust Version: " << std::string(version) << std::endl;
    
    bool ok = kallisto::rust::initialize_security_shell();
    if (ok) {
        std::cout << "[C++] Rust Security Shell initialized successfully!" << std::endl;
    } else {
        std::cout << "[C++] Rust Security Shell initialization failed!" << std::endl;
        return 1;
    }
    
    return 0;
}
