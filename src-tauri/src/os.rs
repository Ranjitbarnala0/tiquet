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
