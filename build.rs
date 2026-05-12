// Embeds assets/icon.ico into the Windows .exe so Explorer, taskbar,
// Alt+Tab, Apps & Features and the MSI installer's About dialog all show
// the proper stock-chart icon instead of the generic Win32 placeholder.
fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            // Don't fail the build if rc.exe/windres is missing — useful when
            // cross-compiling or building docs. The icon just won't be embedded.
            eprintln!("cargo:warning=winresource failed: {e}");
        }
    }
}
