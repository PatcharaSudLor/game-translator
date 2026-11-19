use std::ffi::c_void;
use crate::trace;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_SHOW_TEXT: u32 = WM_APP + 1;
pub const WM_HIDE_TEXT: u32 = WM_APP + 2;
const TIMER_ID: usize = 1;
const PAD: i32 = 14;

pub struct Layout {
    pub screen_w: i32,
    pub screen_h: i32,
    pub font_name: String,
    pub font_px: i32,
    pub opacity: u8,
    pub seconds: u32,
}

static LAYOUT: OnceLock<Layout> = OnceLock::new();
static TEXT: Mutex<String> = Mutex::new(String::new());
static OVERLAY_RAW: AtomicUsize = AtomicUsize::new(0);

fn layout() -> &'static Layout {
    LAYOUT.get().expect("overlay layout")
}

/// HWND ส่งข้ามเธรดตรง ๆ ไม่ได้ จึงส่งเป็นตัวเลข
pub fn to_raw(h: HWND) -> usize {
    h.0 as usize
}
fn from_raw(v: usize) -> HWND {
    HWND(v as *mut c_void)
}

pub fn show_text(raw: usize, text: &str) {
    if let Ok(mut t) = TEXT.lock() {
        *t = text.to_string();
    }
    unsafe {
        let _ = PostMessageW(from_raw(raw), WM_SHOW_TEXT, WPARAM(0), LPARAM(0));
    }
}

/// แสดงข้อความบนโอเวอร์เลย์จากที่ไหนก็ได้ (ใช้ handle ที่เก็บไว้ตอนสร้างหน้าต่าง)
pub fn notify(text: &str) {
    let raw = OVERLAY_RAW.load(Ordering::Relaxed);
    if raw != 0 {
        show_text(raw, text);
    }
}

pub fn notify_hide() {
    let raw = OVERLAY_RAW.load(Ordering::Relaxed);
    if raw != 0 {
        hide(raw);
    }
}

pub fn hide(raw: usize) {
    unsafe {
        let _ = PostMessageW(from_raw(raw), WM_HIDE_TEXT, WPARAM(0), LPARAM(0));
    }
}

pub fn create(l: Layout) -> Result<HWND, String> {
    let _ = LAYOUT.set(l);
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).map_err(|e| e.to_string())?.into();
        let class = w!("GameTranslatorOverlay");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err("RegisterClassW ล้มเหลว".into());
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            class,
            w!("Game Translator"),
            WS_POPUP,
            0,
            0,
            100,
            100,
            None,
            None,
            hinst,
            None,
        )
        .map_err(|e| e.to_string())?;
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), layout().opacity, LWA_ALPHA);
        // ไม่ให้แผ่นคำแปลโผล่ในภาพที่เราจับ (Windows 10 2004+; ถ้าไม่รองรับก็ข้ามไป)
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        OVERLAY_RAW.store(to_raw(hwnd), Ordering::Relaxed);
        Ok(hwnd)
    }
}

pub unsafe fn make_font() -> HFONT {
    let l = layout();
    let name: Vec<u16> = l.font_name.encode_utf16().chain(std::iter::once(0)).collect();
    CreateFontW(
        -l.font_px,
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
        PCWSTR(name.as_ptr()),
    )
}

/// คำนวณขนาดแผ่นตามความยาวข้อความ
unsafe fn measure(text: &str) -> (i32, i32) {
    let l = layout();
    let (_, _, region_w, _) = crate::region::get();
    let width = region_w.clamp(480_i32.min(l.screen_w), l.screen_w - 40);
    let hdc = GetDC(None);
    let font = make_font();
    let old = SelectObject(hdc, font);
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rc = RECT { left: 0, top: 0, right: width - 2 * PAD, bottom: 0 };
    DrawTextW(hdc, &mut wide, &mut rc, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
    SelectObject(hdc, old);
    let _ = DeleteObject(font);
    ReleaseDC(None, hdc);
    let height = (rc.bottom + 2 * PAD).min(l.screen_h - 40);
    (width, height)
}

unsafe fn show_now(hwnd: HWND) {
    let text = TEXT.lock().map(|t| t.clone()).unwrap_or_default();
    if text.is_empty() {
        return;
    }
    trace::log("overlay: show_now measure");
    let l = layout();
    let (region_x, region_y, region_w, region_h) = crate::region::get();
    let (w, h) = measure(&text);
    trace::log(&format!("overlay: measured {w}x{h}"));
    let mut x = region_x + (region_w - w) / 2;
    let mut y = region_y - h - 10;
    if y < 0 {
        y = region_y + region_h + 10; // ไม่มีที่ด้านบน ย้ายไปด้านล่างกรอบ
    }
    x = x.clamp(0, (l.screen_w - w).max(0));
    y = y.clamp(0, (l.screen_h - h).max(0));
    trace::log("overlay: SetWindowPos");
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
    trace::log("overlay: ShowWindow");
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    let _ = InvalidateRect(hwnd, None, TRUE);
    trace::log("overlay: SetTimer");
    SetTimer(hwnd, TIMER_ID, l.seconds * 1000, None);
    trace::log("overlay: show_now done");
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SHOW_TEXT => {
            trace::log("overlay: WM_SHOW_TEXT received");
            show_now(hwnd);
            LRESULT(0)
        }
        WM_HIDE_TEXT | WM_TIMER => {
            let _ = KillTimer(hwnd, TIMER_ID);
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            trace::log("overlay: WM_PAINT begin");
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let text = TEXT.lock().map(|t| t.clone()).unwrap_or_default();
            // ยังไม่มีข้อความ: ไม่ต้องวาดอะไร (ส่งข้อความว่างให้ DrawTextW ทำให้ Windows ล้ม)
            if text.is_empty() {
                let _ = EndPaint(hwnd, &ps);
                trace::log("overlay: WM_PAINT skipped (no text)");
                return LRESULT(0);
            }
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);

            let bg = CreateSolidBrush(COLORREF(0x001C1414)); // BGR: พื้นหลังน้ำเงินเข้ม
            FillRect(hdc, &rc, bg);
            let _ = DeleteObject(bg);

            let font = make_font();
            let old = SelectObject(hdc, font);
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(0x00FFFFFF));
            let mut wide: Vec<u16> = text.encode_utf16().collect();
            let mut inner = RECT {
                left: rc.left + PAD,
                top: rc.top + PAD,
                right: rc.right - PAD,
                bottom: rc.bottom - PAD,
            };
            trace::log("overlay: WM_PAINT DrawTextW");
            DrawTextW(hdc, &mut wide, &mut inner, DT_WORDBREAK | DT_NOPREFIX);
            SelectObject(hdc, old);
            let _ = DeleteObject(font);
            let _ = EndPaint(hwnd, &ps);
            trace::log("overlay: WM_PAINT done");
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
