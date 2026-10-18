use std::path::Path;

// Opens Explorer on a folder, or on its parent with the file selected.
#[cfg(windows)]
pub fn reveal(path: &Path) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (file, params) = if path.is_dir() {
        (path.to_string_lossy().into_owned(), String::new())
    } else {
        ("explorer.exe".to_string(), format!("/select,\"{}\"", path.display()))
    };
    let (verb, file, params) = (wide("open"), wide(&file), wide(&params));
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), params.as_ptr(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

#[cfg(not(windows))]
pub fn reveal(_path: &Path) {}

// a plain Windows message box, for failures before the app has a window
#[cfg(windows)]
pub fn alert(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (title, text) = (wide(title), wide(text));
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

#[cfg(not(windows))]
pub fn alert(_title: &str, _text: &str) {}

// Windows in its registry's own words, e.g. "Windows 10 Pro 24H2 (26100.4349)".
// Windows 11 still says 10 there: a build from 22000 on is 11.
#[cfg(windows)]
pub fn windows_version() -> String {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};

    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let key = wide("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let text = |name: &str| {
        let mut buf = [0u16; 128];
        let mut len = std::mem::size_of_val(&buf) as u32;
        let name = wide(name);
        let ok = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len)
        } == 0;
        if ok {
            String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)])
        } else {
            String::new()
        }
    };
    let mut ubr = 0u32;
    let mut len = 4u32;
    let name = wide("UBR");
    unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut ubr as *mut u32).cast(), &mut len);
    }
    format!("{} {} ({}.{ubr})", text("ProductName"), text("DisplayVersion"), text("CurrentBuildNumber"))
}

#[cfg(not(windows))]
pub fn windows_version() -> String {
    String::new()
}
