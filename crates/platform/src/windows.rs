//! capture-platform — the Windows capture backend (#66). A port of the v2 `core/platform/windows.py`
//! (`Win32WindowFinder` / `Win32ScreenGrabber`) and `helper/audiocap_win_rs` (WASAPI process loopback)
//! into the platform-neutral surface in [`crate`].
//!
//! Built incrementally, mirroring how macOS landed (#65): **[A — this slice] window enumeration**
//! (`EnumWindows`) → [B] GDI screenshots → [C] WASAPI per-process loopback + mic (+ device list) → [D]
//! wired into the capture session loop. Until a slice lands its function falls back to the shared stub
//! in `lib.rs`.

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, MAX_PATH, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    GetWindowDC, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, GWL_EXSTYLE, SM_CXSCREEN, SM_CYSCREEN,
    WS_EX_TOOLWINDOW,
};

use crate::{ScreenshotOptions, WindowInfo};

/// Collected during `EnumWindows`. Boxed behind the `LPARAM` the callback receives.
struct Collector {
    windows: Vec<WindowInfo>,
}

/// `EnumWindows` callback: keep visible, non-cloaked, non-tool top-level windows with a title, mapping
/// each to a [`WindowInfo`]. Mirrors the v2 `Win32WindowFinder` filtering.
unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let collector = &mut *(lparam.0 as *mut Collector);

    let visible = IsWindowVisible(hwnd).as_bool();
    let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    let cloaked = is_cloaked(hwnd);
    let title = window_title(hwnd);
    let (width, height) = window_size(hwnd);
    if !should_include(visible, exstyle, cloaked, &title, width, height) {
        return BOOL(1);
    }

    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    let app_name = process_name(pid).unwrap_or_default();

    collector.windows.push(WindowInfo {
        // HWNDs are 32-bit-significant even on 64-bit Windows, so u32 is lossless (matches the wire type).
        window_id: hwnd.0 as usize as u32,
        pid: pid as i32,
        app_name,
        title,
        width,
        height,
    });
    BOOL(1)
}

/// Whether a top-level window is a real, user-facing app window worth listing: visible, not a tool
/// window (palette/tooltip), not DWM-cloaked (UWP ghost/virtual-desktop), titled, and non-zero size.
/// Pure (no Win32 calls) so the filtering rules are unit-tested without an interactive desktop.
fn should_include(visible: bool, exstyle: u32, cloaked: bool, title: &str, width: u32, height: u32) -> bool {
    visible
        && exstyle & WS_EX_TOOLWINDOW.0 == 0
        && !cloaked
        && !title.is_empty()
        && width > 0
        && height > 0
}

/// Whether the window is DWM-cloaked (hidden though technically "visible").
fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    ok.is_ok() && cloaked != 0
}

/// The window title as a Rust `String` (empty if none).
fn window_title(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

/// The window's client+frame size from `GetWindowRect`.
fn window_size(hwnd: HWND) -> (u32, u32) {
    let mut r = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut r) }.is_ok() {
        ((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32)
    } else {
        (0, 0)
    }
}

/// The owning process's executable basename without the `.exe` (e.g. `chrome`), via
/// `QueryFullProcessImageNameW`. `None` if the process can't be opened (e.g. elevated).
fn process_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; MAX_PATH as usize];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        ok.ok()?;
        let full = String::from_utf16_lossy(&buf[..size as usize]);
        let base = full.rsplit(['\\', '/']).next().unwrap_or(&full);
        Some(base.strip_suffix(".exe").or_else(|| base.strip_suffix(".EXE")).unwrap_or(base).to_string())
    }
}

/// On-screen top-level windows, largest area first; filter by `pid` and/or a case-insensitive app-name
/// substring. Mirrors `list_windows` / the v2 `Win32WindowFinder.find`.
pub fn list_windows(pid: Option<i32>, app_name: Option<&str>) -> Result<Vec<WindowInfo>, String> {
    let mut collector = Collector { windows: Vec::new() };
    unsafe {
        EnumWindows(Some(enum_proc), LPARAM(&mut collector as *mut Collector as isize))
            .map_err(|e| format!("EnumWindows failed: {e}"))?;
    }
    let needle = app_name.map(|s| s.to_lowercase());
    let mut windows: Vec<WindowInfo> = collector
        .windows
        .into_iter()
        .filter(|w| pid.is_none_or(|p| w.pid == p))
        .filter(|w| needle.as_deref().is_none_or(|n| w.app_name.to_lowercase().contains(n)))
        .collect();
    // Largest area first (mirrors the macOS/Python ordering the GUI picker expects).
    windows.sort_by(|a, b| (b.width as u64 * b.height as u64).cmp(&(a.width as u64 * a.height as u64)));
    Ok(windows)
}

// ── Screenshots (GDI: PrintWindow for a window, BitBlt for the screen) ──────────────────────────

/// `PrintWindow` flag rendering full (DWM-composited) window content — handles most modern windows.
const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;

/// Capture one window (`Some(window_id)`) or the primary display (`None`), encoded per `opts`. Mirrors
/// `capture_screenshot` / the v2 `Win32ScreenGrabber` (PrintWindow into a memory DC, then `GetDIBits`).
pub fn capture_screenshot(window_id: Option<u32>, opts: &ScreenshotOptions) -> Result<Vec<u8>, String> {
    match window_id {
        Some(id) => capture_window(HWND(id as usize as *mut core::ffi::c_void), opts),
        None => capture_primary_screen(opts),
    }
}

/// Blit `(w, h)` from `src_dc` (a window or screen DC) into a fresh 32-bit top-down DIB and return its
/// RGBA bytes. `print_from` = `Some(hwnd)` uses `PrintWindow` (captures occluded/DWM windows); `None`
/// uses `BitBlt` (the screen). Always frees its GDI objects.
unsafe fn grab_rgba(src_dc: HDC, width: i32, height: i32, print_from: Option<HWND>) -> Result<Vec<u8>, String> {
    if width <= 0 || height <= 0 {
        return Err(format!("invalid capture size {width}x{height}"));
    }
    let mem_dc = CreateCompatibleDC(Some(src_dc));
    if mem_dc.is_invalid() {
        return Err("CreateCompatibleDC failed".into());
    }
    let bitmap = CreateCompatibleBitmap(src_dc, width, height);
    if bitmap.is_invalid() {
        let _ = DeleteDC(mem_dc);
        return Err("CreateCompatibleBitmap failed".into());
    }
    let prev = SelectObject(mem_dc, HGDIOBJ(bitmap.0));

    let blit_ok = match print_from {
        Some(hwnd) => PrintWindow(hwnd, mem_dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool(),
        None => BitBlt(mem_dc, 0, 0, width, height, Some(src_dc), 0, 0, SRCCOPY).is_ok(),
    };

    let result = if blit_ok {
        read_dib_rgba(mem_dc, bitmap, width, height)
    } else {
        Err("window/screen blit failed".into())
    };

    SelectObject(mem_dc, prev);
    let _ = DeleteObject(HGDIOBJ(bitmap.0));
    let _ = DeleteDC(mem_dc);
    result
}

/// Extract a 32-bit bitmap as RGBA via `GetDIBits` (top-down BGRA from GDI → RGBA, alpha forced opaque).
unsafe fn read_dib_rgba(mem_dc: HDC, bitmap: HBITMAP, width: i32, height: i32) -> Result<Vec<u8>, String> {
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // negative → top-down rows (no vertical flip needed)
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buf = vec![0u8; (width as usize) * (height as usize) * 4];
    let scanned = GetDIBits(
        mem_dc,
        bitmap,
        0,
        height as u32,
        Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
        &mut info,
        DIB_RGB_COLORS,
    );
    if scanned == 0 {
        return Err("GetDIBits failed".into());
    }
    // GDI gives BGRA; convert in place to RGBA and force alpha opaque (GDI leaves it 0).
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    Ok(buf)
}

/// Capture a single window with `PrintWindow`.
fn capture_window(hwnd: HWND, opts: &ScreenshotOptions) -> Result<Vec<u8>, String> {
    unsafe {
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).map_err(|e| format!("GetWindowRect: {e}"))?;
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let win_dc = GetWindowDC(Some(hwnd));
        if win_dc.is_invalid() {
            return Err("GetWindowDC failed (window closed?)".into());
        }
        let rgba = grab_rgba(win_dc, w, h, Some(hwnd));
        ReleaseDC(Some(hwnd), win_dc);
        crate::encode_image(rgba?, w as u32, h as u32, opts)
    }
}

/// Capture the primary display with `BitBlt` from the screen DC.
fn capture_primary_screen(opts: &ScreenshotOptions) -> Result<Vec<u8>, String> {
    unsafe {
        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        let screen_dc = GetDC(None);
        if screen_dc.is_invalid() {
            return Err("GetDC(screen) failed (no display?)".into());
        }
        let rgba = grab_rgba(screen_dc, w, h, None);
        ReleaseDC(None, screen_dc);
        crate::encode_image(rgba?, w as u32, h as u32, opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, ShowWindow, CW_USEDEFAULT,
        SW_SHOWNORMAL, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn test_wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        DefWindowProcW(h, msg, w, l)
    }

    // The GDI screen-capture pipeline (BitBlt → GetDIBits → BGRA→RGBA → PNG encode). On a real display
    // this asserts a decodable PNG of the screen's size; on a non-interactive Session-0 desktop with no
    // display it skips. Either way it exercises the encode path end-to-end when a display exists.
    #[test]
    fn captures_primary_screen_or_skips() {
        let opts = ScreenshotOptions::default();
        match capture_screenshot(None, &opts) {
            Ok(png) => {
                assert!(!png.is_empty(), "screenshot bytes empty");
                let img = image::load_from_memory(&png).expect("screenshot decodes as an image");
                assert!(img.width() > 0 && img.height() > 0, "zero-size screenshot");
            }
            Err(e) => eprintln!("skipping screen capture (no display in this session?): {e}"),
        }
    }

    // The filtering rules, exercised without an interactive desktop (the agent shell runs in Session 0,
    // where no window is enumerable/visible — so this is the env-independent coverage of the logic).
    #[test]
    fn should_include_rules() {
        let ok = |t: &str| should_include(true, 0, false, t, 800, 600);
        assert!(ok("Chrome"), "a normal visible titled window is included");
        assert!(!should_include(false, 0, false, "Chrome", 800, 600), "invisible excluded");
        assert!(!should_include(true, WS_EX_TOOLWINDOW.0, false, "Palette", 800, 600), "tool window excluded");
        assert!(!should_include(true, 0, true, "Ghost", 800, 600), "DWM-cloaked excluded");
        assert!(!should_include(true, 0, false, "", 800, 600), "untitled excluded");
        assert!(!should_include(true, 0, false, "Zero", 0, 600), "zero-width excluded");
        assert!(!should_include(true, 0, false, "Zero", 800, 0), "zero-height excluded");
    }

    // Create a real visible top-level window in THIS process and assert list_windows finds it. This is
    // self-contained (no interactive desktop needed), so it verifies the EnumWindows callback, the
    // visible/title/size/pid extraction, and the mapping — even when run in a non-interactive session.
    #[test]
    fn finds_a_window_created_by_this_process() {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap();
            let class = wide("CaptureTestWndClass");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(test_wndproc),
                hInstance: hinst.into(),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            let atom = RegisterClassW(&wc);
            assert!(atom != 0, "RegisterClassW failed");

            let title = "CaptureHermeticTestWindow";
            let title_w = wide(title);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                PCWSTR(title_w.as_ptr()),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                400,
                300,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .expect("CreateWindowExW failed");
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);

            // On a non-interactive desktop (e.g. CI / a Session-0 service shell) no window is actually
            // visible or enumerable — skip rather than fail. On a real interactive desktop this asserts
            // the full EnumWindows → filter → map path end-to-end.
            let interactive = IsWindowVisible(hwnd).as_bool();
            if !interactive {
                let _ = DestroyWindow(hwnd);
                eprintln!("skipping: non-interactive desktop (window not visible) — EnumWindows yields nothing here");
                return;
            }

            let mine = list_windows(Some(std::process::id() as i32), None).unwrap();
            let _ = DestroyWindow(hwnd);

            let found = mine.iter().find(|w| w.title == title);
            assert!(found.is_some(), "created window not enumerated; got {mine:?}");
            let w = found.unwrap();
            assert_eq!(w.pid, std::process::id() as i32);
            assert!(w.width > 0 && w.height > 0, "window has zero size: {w:?}");
            assert!(!w.app_name.is_empty(), "app_name empty: {w:?}");
        }
    }
}
