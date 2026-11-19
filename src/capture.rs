use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

pub struct Frame {
    pub width: i32,
    pub height: i32,
    /// พิกเซล BGRA (alpha = 255)
    pub bgra: Vec<u8>,
}

pub fn screen_size() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// จับภาพเฉพาะกรอบที่กำหนด (พิกัดจอหลัก) และขยาย `scale` เท่าด้วย GDI
pub fn capture(x: i32, y: i32, w: i32, h: i32, scale: i32) -> Result<Frame, String> {
    if w <= 0 || h <= 0 {
        return Err("กรอบจับภาพมีขนาดไม่ถูกต้อง".into());
    }
    let out_w = w * scale;
    let out_h = h * scale;

    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err("GetDC ล้มเหลว".into());
        }
        let mem = CreateCompatibleDC(screen);

        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = out_w;
        bmi.bmiHeader.biHeight = -out_h; // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;

        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
        let dib = match dib {
            Ok(d) if !bits.is_null() => d,
            _ => {
                let _ = DeleteDC(mem);
                ReleaseDC(None, screen);
                return Err("CreateDIBSection ล้มเหลว".into());
            }
        };

        let old = SelectObject(mem, dib);
        SetStretchBltMode(mem, HALFTONE);
        let _ = SetBrushOrgEx(mem, 0, 0, None);
        let ok = StretchBlt(
            mem, 0, 0, out_w, out_h, screen, x, y, w, h,
            SRCCOPY | CAPTUREBLT,
        );

        let result = if ok.as_bool() {
            let len = (out_w * out_h * 4) as usize;
            let mut buf = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
            for px in buf.chunks_exact_mut(4) {
                px[3] = 255;
            }
            Ok(Frame { width: out_w, height: out_h, bgra: buf })
        } else {
            Err("StretchBlt ล้มเหลว (เกมอาจเป็น Exclusive Fullscreen ให้ใช้ Borderless Window)".to_string())
        };

        SelectObject(mem, old);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    }
}

/// สร้างตารางความสว่างหยาบ ๆ (บล็อก 16px) ไว้เทียบว่าภาพในกรอบเปลี่ยนไหม โดยไม่ต้อง OCR
pub fn luma_grid(f: &Frame) -> (usize, usize, Vec<u8>) {
    const B: usize = 16;
    let w = f.width as usize;
    let h = f.height as usize;
    let gw = (w / B).max(1);
    let gh = (h / B).max(1);
    let mut grid = vec![0u8; gw * gh];
    for gy in 0..gh {
        for gx in 0..gw {
            let mut sum = 0u32;
            let mut n = 0u32;
            let mut y = gy * B;
            while y < ((gy + 1) * B).min(h) {
                let mut x = gx * B;
                while x < ((gx + 1) * B).min(w) {
                    let i = (y * w + x) * 4;
                    // ค่าประมาณความสว่าง (B + 2G + R) / 4
                    sum += (f.bgra[i] as u32 + 2 * f.bgra[i + 1] as u32 + f.bgra[i + 2] as u32) / 4;
                    n += 1;
                    x += 4;
                }
                y += 4;
            }
            grid[gy * gw + gx] = if n > 0 { (sum / n) as u8 } else { 0 };
        }
    }
    (gw, gh, grid)
}

/// บันทึกเป็นไฟล์ BMP (ใช้ตอนตั้งกรอบด้วย F6)
pub fn save_bmp(f: &Frame, path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    let data_len = f.bgra.len() as u32;
    let mut out = Vec::with_capacity(54 + f.bgra.len());
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + data_len).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&f.width.to_le_bytes());
    out.extend_from_slice(&(-f.height).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    out.extend_from_slice(&f.bgra);
    std::fs::File::create(path)?.write_all(&out)
}
