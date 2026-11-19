//! หน้าต่างลากเมาส์เลือกกรอบ: แช่ภาพหน้าจอ ณ ตอนกด F5 แล้วให้ลากครอบข้อความ
use crate::{capture, overlay, region};
use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;
use std::sync::Once;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;

struct Sel {
    sw: i32,
    sh: i32,
    dim_dc: HDC,
    dim_bmp: HBITMAP,
    orig_dc: HDC,
    orig_bmp: HBITMAP,
    back_dc: HDC,
    back_bmp: HBITMAP,
    old: [HGDIOBJ; 3],
    start: (i32, i32),
    cur: (i32, i32),
    dragging: bool,
}

thread_local! {
    // ทุกอย่างทำงานบนเธรดหลัก (เธรดเดียวกับ message loop)
    static SEL: RefCell<Option<Sel>> = RefCell::new(None);
}

static REGISTER: Once = Once::new();

unsafe fn make_dib(w: i32, h: i32) -> Option<(HBITMAP, *mut u8)> {
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB.0;
    let mut bits: *mut c_void = null_mut();
    let bmp = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    if bits.is_null() {
        let _ = DeleteObject(bmp);
        return None;
    }
    Some((bmp, bits as *mut u8))
}

unsafe fn free(st: Sel) {
    SelectObject(st.orig_dc, st.old[0]);
    SelectObject(st.dim_dc, st.old[1]);
    SelectObject(st.back_dc, st.old[2]);
    let _ = DeleteObject(st.orig_bmp);
    let _ = DeleteObject(st.dim_bmp);
    let _ = DeleteObject(st.back_bmp);
    let _ = DeleteDC(st.orig_dc);
    let _ = DeleteDC(st.dim_dc);
    let _ = DeleteDC(st.back_dc);
}

/// เริ่มโหมดเลือกกรอบ (เรียกจากเธรดหลักเมื่อกด F5)
pub fn begin() {
    if SEL.with(|s| s.borrow().is_some()) {
        return;
    }
    overlay::notify_hide();
    let (sw, sh) = capture::screen_size();
    let frame = match capture::capture(0, 0, sw, sh, 1) {
        Ok(f) => f,
        Err(e) => {
            overlay::notify(&format!("[!] จับภาพหน้าจอไม่ได้: {e}"));
            return;
        }
    };

    unsafe {
        let (orig_bmp, orig_bits) = match make_dib(sw, sh) {
            Some(v) => v,
            None => return,
        };
        let (dim_bmp, dim_bits) = match make_dib(sw, sh) {
            Some(v) => v,
            None => {
                let _ = DeleteObject(orig_bmp);
                return;
            }
        };
        let n = (sw as usize) * (sh as usize) * 4;
        std::ptr::copy_nonoverlapping(frame.bgra.as_ptr(), orig_bits, n.min(frame.bgra.len()));
        // ภาพมืดลง 60% ไว้เป็นพื้นหลังของโหมดเลือก
        let dim = std::slice::from_raw_parts_mut(dim_bits, n);
        for (d, s) in dim.chunks_exact_mut(4).zip(frame.bgra.chunks_exact(4)) {
            d[0] = (s[0] as u32 * 40 / 100) as u8;
            d[1] = (s[1] as u32 * 40 / 100) as u8;
            d[2] = (s[2] as u32 * 40 / 100) as u8;
            d[3] = 255;
        }

        let screen = GetDC(None);
        let orig_dc = CreateCompatibleDC(screen);
        let dim_dc = CreateCompatibleDC(screen);
        let back_dc = CreateCompatibleDC(screen);
        let back_bmp = CreateCompatibleBitmap(screen, sw, sh);
        ReleaseDC(None, screen);
        let old = [
            SelectObject(orig_dc, orig_bmp),
            SelectObject(dim_dc, dim_bmp),
            SelectObject(back_dc, back_bmp),
        ];
        let st = Sel {
            sw,
            sh,
            dim_dc,
            dim_bmp,
            orig_dc,
            orig_bmp,
            back_dc,
            back_bmp,
            old,
            start: (0, 0),
            cur: (0, 0),
            dragging: false,
        };

        let hinst: HINSTANCE = match GetModuleHandleW(None) {
            Ok(h) => h.into(),
            Err(_) => {
                free(st);
                return;
            }
        };
        let class = w!("GameTranslatorSelect");
        REGISTER.call_once(|| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
        });
        let hwnd = match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class,
            w!("Select region"),
            WS_POPUP,
            0,
            0,
            sw,
            sh,
            None,
            None,
            hinst,
            None,
        ) {
            Ok(h) => h,
            Err(_) => {
                free(st);
                return;
            }
        };
        SEL.with(|s| *s.borrow_mut() = Some(st));
        region::set_selecting(true);
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(hwnd);
        let _ = UpdateWindow(hwnd);
    }
}

fn pos(lp: LPARAM) -> (i32, i32) {
    let x = (lp.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

/// แปลงจุดเริ่ม/จุดปัจจุบันเป็น (x, y, กว้าง, สูง) ที่อยู่ในจอ
fn norm_rect(a: (i32, i32), b: (i32, i32), sw: i32, sh: i32) -> (i32, i32, i32, i32) {
    let l = a.0.min(b.0).clamp(0, sw);
    let t = a.1.min(b.1).clamp(0, sh);
    let r = a.0.max(b.0).clamp(0, sw);
    let btm = a.1.max(b.1).clamp(0, sh);
    (l, t, r - l, btm - t)
}

unsafe fn paint(hdc: HDC, st: &Sel) {
    let _ = BitBlt(st.back_dc, 0, 0, st.sw, st.sh, st.dim_dc, 0, 0, SRCCOPY);

    if st.dragging {
        let (x, y, w, h) = norm_rect(st.start, st.cur, st.sw, st.sh);
        if w > 0 && h > 0 {
            // ส่วนที่เลือกแสดงภาพสว่างปกติ พร้อมกรอบสีทอง
            let _ = BitBlt(st.back_dc, x, y, w, h, st.orig_dc, x, y, SRCCOPY);
            let brush = CreateSolidBrush(COLORREF(0x0000_D7FF));
            let outer = RECT { left: x, top: y, right: x + w, bottom: y + h };
            FrameRect(st.back_dc, &outer, brush);
            if w > 2 && h > 2 {
                let inner = RECT { left: x + 1, top: y + 1, right: x + w - 1, bottom: y + h - 1 };
                FrameRect(st.back_dc, &inner, brush);
            }
            let _ = DeleteObject(brush);
        }
    }

    // แถบคำแนะนำด้านบน
    let font = overlay::make_font();
    let old = SelectObject(st.back_dc, font);
    let bar_h = 64;
    let bar = RECT { left: 0, top: 0, right: st.sw, bottom: bar_h };
    let bg = CreateSolidBrush(COLORREF(0x0028_1818));
    FillRect(st.back_dc, &bar, bg);
    let _ = DeleteObject(bg);
    SetBkMode(st.back_dc, TRANSPARENT);
    SetTextColor(st.back_dc, COLORREF(0x00FF_FFFF));
    let mut text: Vec<u16> = "ลากเมาส์ครอบข้อความที่ต้องการแปล แล้วปล่อยเมาส์   (Esc หรือคลิกขวา = ยกเลิก)"
        .encode_utf16()
        .collect();
    let mut rc = bar;
    DrawTextW(st.back_dc, &mut text, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    SelectObject(st.back_dc, old);
    let _ = DeleteObject(font);

    let _ = BitBlt(hdc, 0, 0, st.sw, st.sh, st.back_dc, 0, 0, SRCCOPY);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(None, IDC_CROSS).unwrap_or_default());
            LRESULT(1)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            SEL.with(|s| {
                if let Some(st) = s.borrow().as_ref() {
                    paint(hdc, st);
                }
            });
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let p = pos(lp);
            SEL.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.start = p;
                    st.cur = p;
                    st.dragging = true;
                }
            });
            let _ = SetCapture(hwnd);
            let _ = InvalidateRect(hwnd, None, FALSE);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let p = pos(lp);
            let dragging = SEL.with(|s| {
                s.borrow_mut()
                    .as_mut()
                    .map(|st| {
                        if st.dragging {
                            st.cur = p;
                        }
                        st.dragging
                    })
                    .unwrap_or(false)
            });
            if dragging {
                let _ = InvalidateRect(hwnd, None, FALSE);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let p = pos(lp);
            let result = SEL.with(|s| {
                let mut b = s.borrow_mut();
                let st = b.as_mut()?;
                if !st.dragging {
                    return None;
                }
                st.dragging = false;
                st.cur = p;
                Some((norm_rect(st.start, st.cur, st.sw, st.sh), st.sw, st.sh))
            });
            let _ = ReleaseCapture();
            if let Some((r, sw, sh)) = result {
                if r.2 >= 24 && r.3 >= 12 {
                    region::apply(r, sw, sh);
                    let _ = DestroyWindow(hwnd);
                } else {
                    // ลากเล็กเกินไป ให้ลองลากใหม่
                    let _ = InvalidateRect(hwnd, None, FALSE);
                }
            }
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wp.0 == VK_ESCAPE.0 as usize {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let st = SEL.with(|s| s.borrow_mut().take());
            if let Some(st) = st {
                free(st);
            }
            region::set_selecting(false);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
