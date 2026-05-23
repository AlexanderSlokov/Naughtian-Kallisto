#[cxx::bridge(namespace = "kallisto::rust")]
pub mod ffi {
    unsafe extern "C++" {
        include!("kallisto/engine/ffi_cxx_boundary.hpp");
        
        type KallistoCore;

        unsafe fn force_flush_engine(core: *mut KallistoCore) -> bool;
        unsafe fn change_sync_mode_engine(core: *mut KallistoCore, mode: i32) -> bool;
    }

    extern "Rust" {
        type AdminServer;

        fn get_rust_version() -> String;
        fn initialize_security_shell() -> bool;

        unsafe fn start_admin_server(core: *mut KallistoCore, port: u16) -> *mut AdminServer;
        unsafe fn stop_admin_server(server: *mut AdminServer);
    }
}

pub struct AdminServer(pub *mut control_plane::admin_http::AdminServer);

pub fn get_rust_version() -> String {
    format!("Kallisto Rust Security Shell v{}", env!("CARGO_PKG_VERSION"))
}

pub fn initialize_security_shell() -> bool {
    println!("[Rust] Initializing Security Shell (Control Plane)...");
    true
}

pub unsafe fn start_admin_server(core: *mut ffi::KallistoCore, port: u16) -> *mut AdminServer {
    let callbacks = control_plane::admin_http::AdminCallbacks {
        force_flush: |ptr| {
            let core_ptr = ptr as *mut ffi::KallistoCore;
            ffi::force_flush_engine(core_ptr)
        },
        change_sync_mode: |ptr, mode| {
            let core_ptr = ptr as *mut ffi::KallistoCore;
            ffi::change_sync_mode_engine(core_ptr, mode)
        },
    };

    let core_void = core as *mut std::ffi::c_void;
    let control_server = control_plane::admin_http::start_admin_server(core_void, port, callbacks);
    
    let wrapper = Box::new(AdminServer(control_server));
    Box::into_raw(wrapper)
}

pub unsafe fn stop_admin_server(server_ptr: *mut AdminServer) {
    if server_ptr.is_null() {
        return;
    }
    let wrapper = Box::from_raw(server_ptr);
    control_plane::admin_http::stop_admin_server(wrapper.0);
}
