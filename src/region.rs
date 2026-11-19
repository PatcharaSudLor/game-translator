//! กรอบจับภาพที่ใช้งานอยู่ (แชร์ระหว่างเธรด) และขั้นตอนบันทึกลง config.toml
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// (x, y, กว้าง, สูง) หน่วยพิกเซลของจอหลัก
pub type Rect = (i32, i32, i32, i32);

static RECT: Mutex<Rect> = Mutex::new((0, 0, 16, 16));
static SELECTING: AtomicBool = AtomicBool::new(false);

pub fn get() -> Rect {
    *RECT.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn set(r: Rect) {
    *RECT.lock().unwrap_or_else(|e| e.into_inner()) = r;
}

pub fn is_selecting() -> bool {
    SELECTING.load(Ordering::Relaxed)
}

pub fn set_selecting(v: bool) {
    SELECTING.store(v, Ordering::Relaxed);
}

/// ใช้กรอบใหม่ทันที แล้วบันทึกลง config.toml เป็นสัดส่วนของจอ
pub fn apply(r: Rect, screen_w: i32, screen_h: i32) {
    set(r);
    let (x, y, w, h) = r;
    let sw = screen_w.max(1) as f64;
    let sh = screen_h.max(1) as f64;
    let saved = crate::config::save_region(x as f64 / sw, y as f64 / sh, w as f64 / sw, h as f64 / sh);
    let msg = match saved {
        Ok(()) => format!("บันทึกกรอบแล้ว ({w} x {h} px) กด F6 เพื่อทดสอบว่าอ่านข้อความได้ไหม"),
        Err(e) => format!("ใช้กรอบนี้ชั่วคราว แต่บันทึกลง config.toml ไม่ได้: {e}"),
    };
    println!("[region] updated to {}x{} px at ({}, {})", w, h, x, y);
    crate::overlay::notify(&msg);
}
