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

// Opens a link with the program Windows has for it: the browser, the mail
// app, WhatsApp. False when nothing took it.
#[cfg(windows)]
pub fn open(url: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, url) = (wide("open"), wide(url));
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), url.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    r as isize > 32
}

#[cfg(not(windows))]
pub fn open(_url: &str) -> bool {
    false
}

// whether some program on this PC takes links like whatsapp: or mailto:
// (Store apps register theirs here too)
#[cfg(windows)]
pub fn handles(scheme: &str) -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CLASSES_ROOT, RRF_RT_ANY};

    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (key, value) = (wide(scheme), wide("URL Protocol"));
    unsafe {
        RegGetValueW(HKEY_CLASSES_ROOT, key.as_ptr(), value.as_ptr(), RRF_RT_ANY, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) == 0
    }
}

#[cfg(not(windows))]
pub fn handles(_scheme: &str) -> bool {
    false
}

// Puts a file on the clipboard the way Explorer's Copy does, so Ctrl+V in a
// WhatsApp chat or an email attaches it. The clipboard needs a window to
// own it: a message-only one that is never shown.
#[cfg(windows)]
pub fn copy_file(path: &Path) -> bool {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::UI::Shell::DROPFILES;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE};

    const CF_HDROP: u32 = 15;
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    // the list of files: each one ends in a 0, the list in another
    let mut files = wide(&path.to_string_lossy());
    files.push(0);
    let head = std::mem::size_of::<DROPFILES>();
    let bytes = head + files.len() * 2;
    unsafe {
        let class = wide("STATIC");
        let owner = CreateWindowExW(0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
        if owner.is_null() {
            return false;
        }
        let mut ok = false;
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if !mem.is_null() {
            let p = GlobalLock(mem) as *mut u8;
            if !p.is_null() {
                std::ptr::write_bytes(p, 0, bytes);
                let df = p as *mut DROPFILES;
                (*df).pFiles = head as u32;
                (*df).fWide = 1;
                std::ptr::copy_nonoverlapping(files.as_ptr() as *const u8, p.add(head), files.len() * 2);
                GlobalUnlock(mem);
                if OpenClipboard(owner) != 0 {
                    EmptyClipboard();
                    ok = !SetClipboardData(CF_HDROP, mem).is_null();
                    CloseClipboard();
                }
            }
            // once on the clipboard the memory is Windows' to free
            if !ok {
                GlobalFree(mem);
            }
        }
        DestroyWindow(owner);
        ok
    }
}

#[cfg(not(windows))]
pub fn copy_file(_path: &Path) -> bool {
    false
}

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
