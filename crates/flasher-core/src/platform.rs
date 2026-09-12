//! No host configuration changes or driver installation.
#[must_use]
pub const fn usb_guidance() -> &'static str {
    if cfg!(target_os = "windows") {
        "Install a reviewed sunxi-fel executable and libusb runtime. Check the FEL device (VID 1f3a, PID efe8) in Device Manager. Missing WinUSB access requires a deliberate driver choice for that exact device; this app never replaces drivers. Recovery USB networking needs separate validated driver support."
    } else if cfg!(target_os = "macos") {
        "Install a reviewed sunxi-fel executable with libusb. Check the data cable and FEL jumper; close other USB tools. No sudo is used. The upstream RNDIS-only recovery gadget is not a validated macOS transport; CDC-ECM/NCM support is required."
    } else {
        "Install a reviewed sunxi-fel executable with libusb. If access is denied, ask your administrator for narrowly scoped USB permissions for VID 1f3a, PID efe8. Reconnect after permission changes. This app never invokes sudo or installs udev rules."
    }
}
