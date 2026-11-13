//! The libusb feature links a shared system libusb (LGPL-2.1-or-later).
//! libusb1-sys compiles its own copy when that library is missing. Refuse
//! that result so this crate does not produce a statically linked LGPL binary.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_LIBUSB").is_some() {
        require_shared_libusb();
    }
}

fn require_shared_libusb() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=LIBUSB_STATIC");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    println!("cargo:rerun-if-env-changed=VCPKG_INSTALLATION_ROOT");
    if static_link_requested() {
        fail("static linking of libusb is disabled. Link the shared system library.");
    }
    if shared_libusb_found() {
        return;
    }
    fail(
        "the libusb feature needs a shared system libusb 1.0 (LGPL-2.1-or-later).\n\
         Install it, then if a previous attempt compiled the bundled copy, run:\n\
         cargo clean -p libusb1-sys\n\
         Debian/Ubuntu: sudo apt-get install libusb-1.0-0-dev pkg-config\n\
         macOS: brew install libusb pkg-config\n\
         Windows: vcpkg install libusb:x64-windows\n\
         Do not distribute a binary that compiled the bundled libusb sources.",
    );
}

fn fail(message: &str) -> ! {
    panic!("unbound-protocol: {message}");
}

fn static_link_requested() -> bool {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        return matches!(
            std::env::var("LIBUSB_STATIC").ok().as_deref(),
            Some(value) if !value.is_empty() && value != "0"
        );
    }
    std::env::var("CARGO_CFG_TARGET_FEATURE")
        .map(|features| features.split(',').any(|feature| feature == "crt-static"))
        .unwrap_or(false)
}

fn shared_libusb_found() -> bool {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && vcpkg_shared_libusb() {
        return true;
    }
    std::process::Command::new("pkg-config")
        .args(["--exists", "libusb-1.0"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn vcpkg_shared_libusb() -> bool {
    let Ok(root) =
        std::env::var("VCPKG_ROOT").or_else(|_| std::env::var("VCPKG_INSTALLATION_ROOT"))
    else {
        return false;
    };
    let target = std::env::var("TARGET").unwrap_or_default();
    let triplet = if target.contains("aarch64") || target.contains("arm64") {
        "arm64-windows"
    } else if target.contains("i686") {
        "x86-windows"
    } else {
        "x64-windows"
    };
    std::path::Path::new(&root)
        .join("installed")
        .join(triplet)
        .join("bin")
        .join("libusb-1.0.dll")
        .is_file()
}
