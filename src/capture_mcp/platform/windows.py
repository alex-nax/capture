"""Windows platform backend (zero extra dependencies — pure ``ctypes``).

  * `Win32WindowFinder`  — ``EnumWindows`` + ``GetWindowThreadProcessId`` +
    ``QueryFullProcessImageNameW`` for pid/app-name discovery.
  * `Win32ScreenGrabber` — GDI ``BitBlt``/``PrintWindow`` into a bitmap, then
    GDI+ (``gdiplus.dll``, ships with Windows) to scale and encode to
    png/jpg/jpeg/tiff/gif/bmp with optional JPEG quality.
  * `Win32AudioSource`   — per-app WASAPI loopback is not yet wired (feature #21);
    optional ``ffmpeg`` ``dshow`` microphone capture when configured.

Only imported on Windows (the factory in ``__init__.py`` gates by OS); the module
itself imports cleanly elsewhere, but its classes require Win32 APIs at call time.
See ``docs/specs/platform-abstraction.md``.
"""

from __future__ import annotations

import ctypes
import logging
import os
import shutil
from pathlib import Path

from .base import AudioSource, Platform, ScreenGrabber, WindowFinder, WindowRef, fit_box

log = logging.getLogger(__name__)

# --- Win32 bindings (only meaningful on Windows) -----------------------------
# Defined under a guard so the module imports on any OS; the backends below are
# only constructed on Windows (see platform.current()).
if os.name == "nt":
    from ctypes import wintypes

    user32 = ctypes.WinDLL("user32", use_last_error=True)
    gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
    gdiplus = ctypes.WinDLL("gdiplus", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    dwmapi = ctypes.WinDLL("dwmapi", use_last_error=True)

    EnumWindowsProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    class RECT(ctypes.Structure):
        _fields_ = [("left", wintypes.LONG), ("top", wintypes.LONG),
                    ("right", wintypes.LONG), ("bottom", wintypes.LONG)]

    class GUID(ctypes.Structure):
        _fields_ = [("Data1", wintypes.DWORD), ("Data2", wintypes.WORD),
                    ("Data3", wintypes.WORD), ("Data4", ctypes.c_ubyte * 8)]

    class GdiplusStartupInput(ctypes.Structure):
        _fields_ = [("GdiplusVersion", ctypes.c_uint),
                    ("DebugEventCallback", ctypes.c_void_p),
                    ("SuppressBackgroundThread", wintypes.BOOL),
                    ("SuppressExternalCodecs", wintypes.BOOL)]

    class ImageCodecInfo(ctypes.Structure):
        _fields_ = [
            ("Clsid", GUID), ("FormatID", GUID),
            ("CodecName", wintypes.LPCWSTR), ("DllName", wintypes.LPCWSTR),
            ("FormatDescription", wintypes.LPCWSTR), ("FilenameExtension", wintypes.LPCWSTR),
            ("MimeType", wintypes.LPCWSTR), ("Flags", wintypes.DWORD),
            ("Version", wintypes.DWORD), ("SigCount", wintypes.DWORD),
            ("SigSize", wintypes.DWORD), ("SigPattern", ctypes.POINTER(ctypes.c_ubyte)),
            ("SigMask", ctypes.POINTER(ctypes.c_ubyte)),
        ]

    class EncoderParameter(ctypes.Structure):
        _fields_ = [("Guid", GUID), ("NumberOfValues", ctypes.c_ulong),
                    ("Type", ctypes.c_ulong), ("Value", ctypes.c_void_p)]

    class EncoderParameters(ctypes.Structure):
        _fields_ = [("Count", ctypes.c_uint), ("Parameter", EncoderParameter * 1)]

    def _sig(fn, argtypes, restype):
        fn.argtypes = argtypes
        fn.restype = restype

    _VOID = ctypes.c_void_p
    _INT = ctypes.c_int
    _PVOID = ctypes.POINTER(ctypes.c_void_p)

    _sig(user32.IsWindowVisible, [wintypes.HWND], wintypes.BOOL)
    _sig(user32.GetWindow, [wintypes.HWND, wintypes.UINT], wintypes.HWND)
    _sig(user32.GetWindowLongW, [wintypes.HWND, _INT], wintypes.LONG)
    _sig(user32.GetWindowRect, [wintypes.HWND, ctypes.POINTER(RECT)], wintypes.BOOL)
    _sig(user32.GetWindowTextLengthW, [wintypes.HWND], _INT)
    _sig(user32.GetWindowTextW, [wintypes.HWND, wintypes.LPWSTR, _INT], _INT)
    _sig(user32.GetWindowThreadProcessId,
         [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)], wintypes.DWORD)
    _sig(user32.EnumWindows, [EnumWindowsProc, wintypes.LPARAM], wintypes.BOOL)
    _sig(user32.GetSystemMetrics, [_INT], _INT)
    _sig(user32.GetDC, [wintypes.HWND], wintypes.HDC)
    _sig(user32.GetWindowDC, [wintypes.HWND], wintypes.HDC)
    _sig(user32.ReleaseDC, [wintypes.HWND, wintypes.HDC], _INT)
    _sig(user32.PrintWindow, [wintypes.HWND, wintypes.HDC, wintypes.UINT], wintypes.BOOL)

    _sig(gdi32.CreateCompatibleDC, [wintypes.HDC], wintypes.HDC)
    _sig(gdi32.CreateCompatibleBitmap, [wintypes.HDC, _INT, _INT], wintypes.HBITMAP)
    _sig(gdi32.SelectObject, [wintypes.HDC, wintypes.HGDIOBJ], wintypes.HGDIOBJ)
    _sig(gdi32.BitBlt, [wintypes.HDC, _INT, _INT, _INT, _INT, wintypes.HDC, _INT, _INT,
                        wintypes.DWORD], wintypes.BOOL)
    _sig(gdi32.DeleteObject, [wintypes.HGDIOBJ], wintypes.BOOL)
    _sig(gdi32.DeleteDC, [wintypes.HDC], wintypes.BOOL)

    _sig(kernel32.OpenProcess, [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD], wintypes.HANDLE)
    _sig(kernel32.CloseHandle, [wintypes.HANDLE], wintypes.BOOL)
    _sig(kernel32.QueryFullProcessImageNameW,
         [wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)],
         wintypes.BOOL)

    _sig(dwmapi.DwmGetWindowAttribute,
         [wintypes.HWND, wintypes.DWORD, _VOID, wintypes.DWORD], ctypes.c_long)

    _sig(gdiplus.GdiplusStartup,
         [_PVOID, ctypes.POINTER(GdiplusStartupInput), _VOID], _INT)
    _sig(gdiplus.GdipCreateBitmapFromHBITMAP, [wintypes.HBITMAP, _VOID, _PVOID], _INT)
    _sig(gdiplus.GdipCreateBitmapFromScan0, [_INT, _INT, _INT, _INT, _VOID, _PVOID], _INT)
    _sig(gdiplus.GdipGetImageGraphicsContext, [_VOID, _PVOID], _INT)
    _sig(gdiplus.GdipSetInterpolationMode, [_VOID, _INT], _INT)
    _sig(gdiplus.GdipDrawImageRectI, [_VOID, _VOID, _INT, _INT, _INT, _INT], _INT)
    _sig(gdiplus.GdipDeleteGraphics, [_VOID], _INT)
    _sig(gdiplus.GdipDisposeImage, [_VOID], _INT)
    _sig(gdiplus.GdipGetImageEncodersSize, [ctypes.POINTER(ctypes.c_uint),
                                            ctypes.POINTER(ctypes.c_uint)], _INT)
    _sig(gdiplus.GdipGetImageEncoders, [ctypes.c_uint, ctypes.c_uint, _VOID], _INT)
    _sig(gdiplus.GdipSaveImageToFile, [_VOID, wintypes.LPCWSTR, ctypes.POINTER(GUID), _VOID], _INT)

    # Constants.
    GWL_EXSTYLE = -20
    WS_EX_TOOLWINDOW = 0x00000080
    GW_OWNER = 4
    DWMWA_CLOAKED = 14
    SM_CXSCREEN, SM_CYSCREEN = 0, 1
    SRCCOPY = 0x00CC0020
    CAPTUREBLT = 0x40000000
    PW_RENDERFULLCONTENT = 0x00000002
    PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
    PIXELFORMAT_32BPP_ARGB = 0x0026200A
    INTERPOLATION_HQ_BICUBIC = 7
    ENCODER_PARAM_LONG = 4

    def _make_guid(s: str) -> "GUID":
        parts = s.strip("{}").split("-")
        g = GUID()
        g.Data1 = int(parts[0], 16)
        g.Data2 = int(parts[1], 16)
        g.Data3 = int(parts[2], 16)
        for i, b in enumerate(bytes.fromhex(parts[3] + parts[4])):
            g.Data4[i] = b
        return g

    ENCODER_QUALITY = _make_guid("1d5be4b5-fa4a-452d-9cdd-5db35105e7eb")


_MIME = {
    "png": "image/png",
    "jpg": "image/jpeg",
    "jpeg": "image/jpeg",
    "bmp": "image/bmp",
    "gif": "image/gif",
    "tiff": "image/tiff",
}


def _window_text(hwnd: int) -> str:
    n = user32.GetWindowTextLengthW(hwnd)
    if n <= 0:
        return ""
    buf = ctypes.create_unicode_buffer(n + 1)
    user32.GetWindowTextW(hwnd, buf, n + 1)
    return buf.value


def _process_name(pid: int) -> str:
    h = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not h:
        return ""
    try:
        size = wintypes.DWORD(512)
        buf = ctypes.create_unicode_buffer(size.value)
        if kernel32.QueryFullProcessImageNameW(h, 0, buf, ctypes.byref(size)):
            return Path(buf.value).name  # e.g. "chrome.exe"
        return ""
    finally:
        kernel32.CloseHandle(h)


def _is_cloaked(hwnd: int) -> bool:
    val = wintypes.DWORD(0)
    try:
        hr = dwmapi.DwmGetWindowAttribute(
            hwnd, DWMWA_CLOAKED, ctypes.byref(val), ctypes.sizeof(val)
        )
    except OSError:
        return False
    return hr == 0 and val.value != 0


class Win32WindowFinder(WindowFinder):
    def find(self, pid: int | None = None, app_name: str | None = None) -> list[WindowRef]:
        needle = app_name.lower() if app_name else None
        results: list[WindowRef] = []

        def _cb(hwnd, _lparam):
            try:
                if not user32.IsWindowVisible(hwnd):
                    return True
                if user32.GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW:
                    return True
                if _is_cloaked(hwnd):
                    return True
                rect = RECT()
                if not user32.GetWindowRect(hwnd, ctypes.byref(rect)):
                    return True
                w = rect.right - rect.left
                h = rect.bottom - rect.top
                if w < 1 or h < 1:
                    return True
                wpid = wintypes.DWORD(0)
                user32.GetWindowThreadProcessId(hwnd, ctypes.byref(wpid))
                wpid = wpid.value
                if pid is not None and wpid != pid:
                    return True
                title = _window_text(hwnd)
                exe = _process_name(wpid)
                if needle is not None and needle not in f"{exe} {title}".lower():
                    return True
                results.append(
                    WindowRef(
                        window_id=int(hwnd) if hwnd else 0,
                        pid=wpid,
                        app_name=exe,
                        title=title,
                        width=w,
                        height=h,
                    )
                )
            except Exception:  # never let one bad window abort enumeration
                log.exception("EnumWindows callback failed for a window")
            return True

        user32.EnumWindows(EnumWindowsProc(_cb), 0)
        results.sort(key=lambda r: r.area, reverse=True)
        return results


class Win32ScreenGrabber(ScreenGrabber):
    def __init__(self) -> None:
        import threading

        self._lock = threading.Lock()
        self._token: ctypes.c_void_p | None = None
        self._encoders: dict[str, GUID] = {}

    def _ensure_gdiplus(self) -> None:
        with self._lock:
            if self._token is not None:
                return
            token = ctypes.c_void_p()
            startup = GdiplusStartupInput(1, None, 0, 0)
            status = gdiplus.GdiplusStartup(ctypes.byref(token), ctypes.byref(startup), None)
            if status != 0:
                raise RuntimeError(f"GdiplusStartup failed (status={status})")
            self._token = token

    def _encoder_clsid(self, mime: str) -> "GUID | None":
        with self._lock:
            cached = self._encoders.get(mime)
        if cached is not None:
            return cached
        num = ctypes.c_uint(0)
        size = ctypes.c_uint(0)
        if gdiplus.GdipGetImageEncodersSize(ctypes.byref(num), ctypes.byref(size)) != 0 or size.value == 0:
            return None
        buf = (ctypes.c_byte * size.value)()
        if gdiplus.GdipGetImageEncoders(num.value, size.value, buf) != 0:
            return None
        codecs = ctypes.cast(buf, ctypes.POINTER(ImageCodecInfo))
        for i in range(num.value):
            mt = codecs[i].MimeType
            if mt and mt.lower() == mime:
                clsid = GUID()
                ctypes.memmove(ctypes.byref(clsid), ctypes.byref(codecs[i].Clsid), ctypes.sizeof(GUID))
                with self._lock:  # guard the shared cache (contract: backends are thread-safe)
                    self._encoders[mime] = clsid
                return clsid
        return None

    def capture(
        self,
        window_id: int | None,
        out_path: Path,
        *,
        fmt: str,
        resolution: tuple[int, int] | None = None,
        jpeg_quality: int | None = None,
        timeout: float | None = None,
    ) -> bool:
        mime = _MIME.get(fmt if fmt not in ("jpg",) else "jpeg")
        if mime is None:
            log.warning("unsupported screenshot format %r on Windows", fmt)
            return False
        try:
            self._ensure_gdiplus()
            return self._capture(window_id, out_path, fmt, resolution, jpeg_quality, mime)
        except Exception:
            log.exception("Windows screenshot capture failed")
            return False

    def _capture(self, window_id, out_path, fmt, resolution, jpeg_quality, mime) -> bool:
        if window_id is not None:
            hwnd = window_id
            rect = RECT()
            if not user32.GetWindowRect(hwnd, ctypes.byref(rect)):
                log.warning("GetWindowRect failed for hwnd=%s", hwnd)
                return False
            w, h = rect.right - rect.left, rect.bottom - rect.top
            src_dc = user32.GetWindowDC(hwnd)
            dc_owner = hwnd
        else:
            hwnd = None
            w = user32.GetSystemMetrics(SM_CXSCREEN)
            h = user32.GetSystemMetrics(SM_CYSCREEN)
            src_dc = user32.GetDC(None)
            dc_owner = None
        if w < 1 or h < 1 or not src_dc:
            if src_dc:
                user32.ReleaseDC(dc_owner, src_dc)
            log.warning("nothing to capture (w=%s h=%s dc=%s)", w, h, bool(src_dc))
            return False

        mem_dc = gdi32.CreateCompatibleDC(src_dc)
        bmp = gdi32.CreateCompatibleBitmap(src_dc, w, h)
        old = gdi32.SelectObject(mem_dc, bmp)
        gp_img = ctypes.c_void_p()
        scaled = ctypes.c_void_p()
        try:
            if hwnd is not None:
                ok = user32.PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT)
                if not ok:
                    gdi32.BitBlt(mem_dc, 0, 0, w, h, src_dc, 0, 0, SRCCOPY)
            else:
                gdi32.BitBlt(mem_dc, 0, 0, w, h, src_dc, 0, 0, SRCCOPY | CAPTUREBLT)

            # Deselect the bitmap from the DC before handing it to GDI+: per the
            # Bitmap::FromHBITMAP contract a GDI bitmap must NOT be selected into a DC
            # when passed to GdipCreateBitmapFromHBITMAP. The finally re-selects `old`
            # (a harmless no-op) before deleting the bitmap.
            gdi32.SelectObject(mem_dc, old)

            if gdiplus.GdipCreateBitmapFromHBITMAP(bmp, None, ctypes.byref(gp_img)) != 0:
                log.warning("GdipCreateBitmapFromHBITMAP failed")
                return False

            image = gp_img
            if resolution is not None:
                tw, th = fit_box(w, h, *resolution)
                if (tw, th) != (w, h):
                    if gdiplus.GdipCreateBitmapFromScan0(
                        tw, th, 0, PIXELFORMAT_32BPP_ARGB, None, ctypes.byref(scaled)
                    ) != 0:
                        log.warning("GdipCreateBitmapFromScan0 failed")
                        return False
                    g = ctypes.c_void_p()
                    if gdiplus.GdipGetImageGraphicsContext(scaled, ctypes.byref(g)) != 0:
                        log.warning("GdipGetImageGraphicsContext failed; cannot scale to %dx%d", tw, th)
                        return False  # do not silently emit a full-resolution image
                    gdiplus.GdipSetInterpolationMode(g, INTERPOLATION_HQ_BICUBIC)
                    draw = gdiplus.GdipDrawImageRectI(g, gp_img, 0, 0, tw, th)
                    gdiplus.GdipDeleteGraphics(g)
                    if draw != 0:
                        log.warning("GdipDrawImageRectI failed (status=%s)", draw)
                        return False
                    image = scaled

            clsid = self._encoder_clsid(mime)
            if clsid is None:
                log.warning("no GDI+ encoder for %s", mime)
                return False

            params_ptr = None
            quality = None  # keep alive until after Save
            eps = None
            if fmt in ("jpg", "jpeg") and jpeg_quality is not None:
                quality = ctypes.c_ulong(max(0, min(100, int(jpeg_quality))))
                eps = EncoderParameters()
                eps.Count = 1
                eps.Parameter[0].Guid = ENCODER_QUALITY
                eps.Parameter[0].NumberOfValues = 1
                eps.Parameter[0].Type = ENCODER_PARAM_LONG
                eps.Parameter[0].Value = ctypes.cast(ctypes.byref(quality), ctypes.c_void_p)
                params_ptr = ctypes.cast(ctypes.byref(eps), ctypes.c_void_p)

            status = gdiplus.GdipSaveImageToFile(image, str(out_path), ctypes.byref(clsid), params_ptr)
            if status != 0:
                log.warning("GdipSaveImageToFile failed (status=%s) for %s", status, out_path)
                return False
            return out_path.exists()
        finally:
            if gp_img:
                gdiplus.GdipDisposeImage(gp_img)
            if scaled:
                gdiplus.GdipDisposeImage(scaled)
            gdi32.SelectObject(mem_dc, old)
            gdi32.DeleteObject(bmp)
            gdi32.DeleteDC(mem_dc)
            user32.ReleaseDC(dc_owner, src_dc)


class Win32AudioSource(AudioSource):
    def command(
        self,
        *,
        pid: int | None,
        bundle_id: str | None,
        source: str,
        rate: int,
    ) -> tuple[list[str], str] | None:
        # Per-app audio via WASAPI process loopback is not yet implemented on
        # Windows (feature #21) — there is no helper analogous to audiocap yet.
        if source == "app":
            return None
        # Optional microphone capture via ffmpeg dshow. dshow has no ":default"
        # device, so a device name must be supplied via CAPTURE_DSHOW_AUDIO.
        if source in ("auto", "mic") and shutil.which("ffmpeg"):
            device = os.environ.get("CAPTURE_DSHOW_AUDIO")
            if device:
                return (
                    [
                        "ffmpeg", "-hide_banner", "-loglevel", "warning",
                        "-f", "dshow", "-i", f"audio={device}",
                        "-ac", "1", "-ar", str(rate),
                        "-f", "s16le", "-",
                    ],
                    "mic",
                )
        return None


class WindowsPlatform(Platform):
    name = "windows"

    def __init__(self) -> None:
        super().__init__(Win32WindowFinder(), Win32ScreenGrabber(), Win32AudioSource())
