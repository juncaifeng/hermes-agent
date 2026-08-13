//! Quick-screenshot commands for the composer: enumerate the user's visible
//! windows and capture one (or the whole virtual screen) to a PNG.
//!
//! The captured pixels never touch the Python backend — the annotated image
//! is inserted through the normal composer-attachment channel
//! (`save_image_buffer` in desktop_misc.rs). Windows-only: other platforms
//! get an explicit "unsupported" error instead of an empty list so the UI can
//! hide the entry point.

use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct WindowInfo {
    /// Decimal HWND string — pass back to `capture_window`. `"screen"` is the
    /// renderer-side sentinel for the full virtual screen; it never appears
    /// in this list.
    pub id: String,
    pub title: String,
    /// Process image file name (e.g. "notepad.exe"); empty when the process
    /// name can't be read (elevated/system processes).
    pub process: String,
    pub pid: u32,
}

#[cfg(windows)]
mod imp {
    use super::WindowInfo;
    use std::path::Path;

    use windows::core::BOOL;
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
        HGDIOBJ, SRCCOPY,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
        GetWindowThreadProcessId, IsWindow, IsWindowVisible, GWL_EXSTYLE, PW_RENDERFULLCONTENT,
        SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        WS_EX_TOOLWINDOW,
    };

    fn window_title(hwnd: HWND) -> String {
        let mut buf = [0u16; 512];
        let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
        if len <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize]).trim().to_string()
    }

    fn process_image_name(pid: u32) -> String {
        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return String::new();
            };
            let mut buf = [0u16; 1024];
            let mut size = buf.len() as u32;
            let name = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_FORMAT(0), // Win32 path form
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut size,
            )
            .ok()
            .map(|()| {
                let full = String::from_utf16_lossy(&buf[..size as usize]);
                Path::new(&full)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
            let _ = CloseHandle(handle);
            name
        }
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam.0 as *mut Vec<WindowInfo>);
        let own_pid = std::process::id();

        if IsWindowVisible(hwnd).as_bool() {
            let title = window_title(hwnd);
            // Tool windows (floating palettes) and empty-title windows are
            // noise for a "which app" picker.
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let is_tool = ex_style & (WS_EX_TOOLWINDOW.0 as isize) != 0;

            if !title.is_empty() && !is_tool {
                let mut pid: u32 = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                // Never list our own windows — self-screenshots are a
                // feedback loop (picker/preview capture) with no use case.
                if pid != 0 && pid != own_pid {
                    out.push(WindowInfo {
                        id: (hwnd.0 as isize).to_string(),
                        title,
                        process: process_image_name(pid),
                        pid,
                    });
                }
            }
        }

        BOOL(1) // continue enumeration
    }

    pub fn list_windows() -> Result<Vec<WindowInfo>, String> {
        let mut out: Vec<WindowInfo> = Vec::new();
        unsafe {
            EnumWindows(
                Some(enum_proc),
                LPARAM(&mut out as *mut Vec<WindowInfo> as isize),
            )
            .map_err(|e| format!("EnumWindows failed: {e}"))?;
        }
        Ok(out)
    }

    struct DcGuard(HDC);
    impl Drop for DcGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = DeleteDC(self.0);
            }
        }
    }

    struct BitmapGuard(HBITMAP);
    impl Drop for BitmapGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = DeleteObject(HGDIOBJ((self.0).0));
            }
        }
    }

    /// Read the current contents of `memdc`'s selected bitmap as RGBA pixels.
    fn bitmap_to_rgba(memdc: HDC, bitmap: HBITMAP, width: i32, height: i32) -> Result<Vec<u8>, String> {
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative height = top-down row order.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bgra = vec![0u8; (width * height * 4) as usize];
        let rows = unsafe {
            GetDIBits(
                memdc,
                bitmap,
                0,
                height as u32,
                Some(bgra.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        };
        if rows == 0 {
            return Err("reading captured pixels failed".to_string());
        }
        // GDI gives BGRA; PNG wants RGBA.
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        Ok(bgra)
    }

    fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<String, String> {
        use base64::Engine as _;

        let img = image::RgbaImage::from_raw(width, height, rgba.to_vec())
            .ok_or_else(|| "captured image has invalid dimensions".to_string())?;
        let mut png_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .map_err(|e| format!("PNG encode failed: {e}"))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(&png_bytes))
    }

    fn all_zero(pixels: &[u8]) -> bool {
        pixels.iter().all(|&b| b == 0)
    }

    fn capture_rect(x: i32, y: i32, width: i32, height: i32, from: Option<HWND>) -> Result<String, String> {
        if width <= 0 || height <= 0 {
            return Err(
                "capture produced an empty image — the window may be minimized; restore it and try again"
                    .to_string(),
            );
        }

        unsafe {
            let screen_dc = match from {
                Some(hwnd) => GetDC(Some(hwnd)),
                None => GetDC(None),
            };
            if screen_dc.0.is_null() {
                return Err("no display context available".to_string());
            }

            let result = (|| {
                let memdc = CreateCompatibleDC(Some(screen_dc));
                if memdc.0.is_null() {
                    return Err("CreateCompatibleDC failed".to_string());
                }
                let _dc_guard = DcGuard(memdc);

                let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
                if bitmap.0.is_null() {
                    return Err("CreateCompatibleBitmap failed".to_string());
                }
                let _bmp_guard = BitmapGuard(bitmap);

                let old = SelectObject(memdc, HGDIOBJ(bitmap.0));

                let painted = match from {
                    // PrintWindow redraws even occluded windows; its return
                    // value is unreliable (some apps answer 0 but paint fine),
                    // so judge by the pixels, not the BOOL.
                    Some(hwnd) => {
                        let _ = PrintWindow(hwnd, memdc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT));
                        true
                    }
                    None => BitBlt(memdc, 0, 0, width, height, Some(screen_dc), x, y, SRCCOPY).is_ok(),
                };
                SelectObject(memdc, old);

                if !painted {
                    return Err("capture failed".to_string());
                }

                let rgba = bitmap_to_rgba(memdc, bitmap, width, height)?;
                if all_zero(&rgba) {
                    return Err(
                        "capture produced an empty image — the window may be minimized; restore it and try again"
                            .to_string(),
                    );
                }
                encode_png(&rgba, width as u32, height as u32)
            })();

            ReleaseDC(from, screen_dc);
            result
        }
    }

    pub fn capture_window(id: &str) -> Result<String, String> {
        if id == "screen" {
            unsafe {
                let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
                let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
                let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
                let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
                return capture_rect(x, y, w, h, None);
            }
        }

        let raw: isize = id
            .parse()
            .map_err(|_| format!("invalid window id '{id}'"))?;
        let hwnd = HWND(raw as *mut _);
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                return Err("that window no longer exists".to_string());
            }
            let mut rect = RECT::default();
            GetWindowRect(hwnd, &mut rect).map_err(|e| format!("GetWindowRect failed: {e}"))?;
            capture_rect(
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                Some(hwnd),
            )
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::WindowInfo;

    pub fn list_windows() -> Result<Vec<WindowInfo>, String> {
        Err("window capture is unsupported on this platform".to_string())
    }

    pub fn capture_window(_id: &str) -> Result<String, String> {
        Err("window capture is unsupported on this platform".to_string())
    }
}

/// `hermes:listWindows` — visible top-level windows, minus our own process.
#[tauri::command]
pub fn list_windows() -> Result<Vec<WindowInfo>, String> {
    imp::list_windows()
}

/// `hermes:captureWindow` — base64 PNG of one window (or `"screen"` for the
/// full virtual screen). Async: the full-screen path briefly hides the
/// quick-capture overlay and sleeps a beat for the compositor — that wait
/// must not sit on the main thread.
#[tauri::command]
pub async fn capture_window(app: tauri::AppHandle, id: String) -> Result<String, String> {
    if id == "screen" {
        return crate::screenshots::with_overlay_hidden(&app, || imp::capture_window(&id));
    }
    imp::capture_window(&id)
}
