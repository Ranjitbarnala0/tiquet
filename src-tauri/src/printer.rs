use std::path::Path;

use serde::Serialize;

use crate::error::{fail, Error, Result};
use crate::state::PrintSink;

#[derive(Serialize)]
pub struct Printers {
    pub printers: Vec<String>,
    pub default: Option<String>,
}

pub fn list() -> Printers {
    let (printers, default) = win::list();
    Printers { printers, default }
}

pub fn send(sink: &PrintSink, printer: &str, doc: &str, data: &[u8]) -> Result<()> {
    match sink {
        PrintSink::Dir(dir) => {
            std::fs::create_dir_all(dir)?;
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%3f");
            let name: String = doc
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
                .collect();
            std::fs::write(dir.join(format!("{stamp}-{name}.bin")), data)?;
            Ok(())
        }
        PrintSink::Spooler => {
            if printer.trim().is_empty() {
                return fail("E_NO_PRINTER");
            }
            raw(printer, doc, data, None)
        }
    }
}

pub fn raw(printer: &str, doc: &str, data: &[u8], out_file: Option<&Path>) -> Result<()> {
    win::raw(printer, doc, data, out_file).map_err(|e| Error(format!("E_PRINT:{e}")))
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Graphics::Printing::{
        ClosePrinter, EndDocPrinter, EndPagePrinter, EnumPrintersW, GetDefaultPrinterW, GetPrinterDriverW,
        OpenPrinterW, StartDocPrinterW, StartPagePrinter, WritePrinter, DOC_INFO_1W, DRIVER_INFO_2W,
        PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_INFO_4W,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn from_wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }

    fn last_error() -> String {
        std::io::Error::last_os_error().to_string()
    }

    pub fn list() -> (Vec<String>, Option<String>) {
        let mut names = Vec::new();
        let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
        unsafe {
            let (mut needed, mut count) = (0u32, 0u32);
            EnumPrintersW(flags, null(), 4, null_mut(), 0, &mut needed, &mut count);
            if needed > 0 {
                // u64 backing keeps the PRINTER_INFO_4W array pointer-aligned
                let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
                if EnumPrintersW(flags, null(), 4, buf.as_mut_ptr() as *mut u8, needed, &mut needed, &mut count) != 0 {
                    let infos = std::slice::from_raw_parts(buf.as_ptr() as *const PRINTER_INFO_4W, count as usize);
                    for i in infos {
                        names.push(from_wide(i.pPrinterName));
                    }
                }
            }
        }
        names.sort_by_key(|n| n.to_lowercase());
        (names, default_printer())
    }

    fn default_printer() -> Option<String> {
        unsafe {
            let mut len = 0u32;
            GetDefaultPrinterW(null_mut(), &mut len);
            if len == 0 {
                return None;
            }
            let mut buf = vec![0u16; len as usize];
            if GetDefaultPrinterW(buf.as_mut_ptr(), &mut len) == 0 {
                return None;
            }
            Some(from_wide(buf.as_ptr()))
        }
    }

    unsafe fn driver_version(h: HANDLE) -> u32 {
        let mut needed = 0u32;
        GetPrinterDriverW(h, null(), 2, null_mut(), 0, &mut needed);
        if needed == 0 {
            return 3;
        }
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        if GetPrinterDriverW(h, null(), 2, buf.as_mut_ptr() as *mut u8, needed, &mut needed) == 0 {
            return 3;
        }
        (*(buf.as_ptr() as *const DRIVER_INFO_2W)).cVersion
    }

    // RAW hands the bytes straight to the port on classic (v3) drivers. A v4
    // (XPS) driver accepts a RAW job and then silently drops it, so those get
    // XPS_PASS, their pass-through datatype.
    pub fn raw(printer: &str, doc: &str, data: &[u8], out_file: Option<&Path>) -> Result<(), String> {
        let name = wide(printer);
        let doc_w = wide(doc);
        let out_w = out_file.map(|p| wide(&p.to_string_lossy()));
        unsafe {
            let mut h: HANDLE = null_mut();
            if OpenPrinterW(name.as_ptr(), &mut h, null()) == 0 {
                return Err(last_error());
            }
            let mut first_err = None;
            let mut result = Err(String::new());
            let order = if driver_version(h) >= 4 { ["XPS_PASS", "RAW"] } else { ["RAW", "XPS_PASS"] };
            for dtype in order {
                let dtype_w = wide(dtype);
                let info = DOC_INFO_1W {
                    pDocName: doc_w.as_ptr() as *mut u16,
                    pOutputFile: out_w.as_ref().map_or(null_mut(), |o| o.as_ptr() as *mut u16),
                    pDatatype: dtype_w.as_ptr() as *mut u16,
                };
                if StartDocPrinterW(h, 1, &info) == 0 {
                    first_err.get_or_insert_with(last_error);
                    continue;
                }
                result = write_job(h, data);
                EndDocPrinter(h);
                break;
            }
            ClosePrinter(h);
            match (result, first_err) {
                (Err(e), Some(first)) if e.is_empty() => Err(first),
                (r, _) => r,
            }
        }
    }

    unsafe fn write_job(h: HANDLE, data: &[u8]) -> Result<(), String> {
        if StartPagePrinter(h) == 0 {
            return Err(last_error());
        }
        let mut written = 0u32;
        let ok = WritePrinter(h, data.as_ptr() as *const c_void, data.len() as u32, &mut written);
        let err = (ok == 0).then(last_error);
        EndPagePrinter(h);
        match err {
            Some(e) => Err(e),
            None if written as usize != data.len() => Err(format!("{written}/{} bytes", data.len())),
            None => Ok(()),
        }
    }
}

#[cfg(not(windows))]
mod win {
    use std::path::Path;

    pub fn list() -> (Vec<String>, Option<String>) {
        (Vec::new(), None)
    }

    pub fn raw(_: &str, _: &str, _: &[u8], _: Option<&Path>) -> Result<(), String> {
        Err("printing needs Windows".into())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::time::Duration;

    // Every Windows install ships "Microsoft Print to PDF". With an output
    // file set the spooler writes the job there instead of prompting, so the
    // whole OpenPrinter/StartDoc/WritePrinter path runs without paper.
    #[test]
    #[ignore = "talks to the Windows spooler"]
    fn spooler_passes_bytes_through_untouched() {
        let out = std::env::temp_dir().join(format!("tiquet-spool-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        super::raw("Microsoft Print to PDF", "tiquet-test", &data, Some(&out)).unwrap();
        for _ in 0..100 {
            if std::fs::metadata(&out).is_ok_and(|m| m.len() as usize >= data.len()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(std::fs::read(&out).unwrap(), data);
        assert!(super::list().printers.iter().any(|p| p == "Microsoft Print to PDF"));
    }
}
