use crate::capture::{self, Frame};
use crate::config::Config;
use crate::ocr::Ocr;
use crate::overlay;
use crate::region;
use crate::trace;
use crate::translate::{TrError, Translator};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

pub enum Cmd {
    /// กด F8: แปลทันที
    Translate,
    /// กด F7: แปล + อธิบายศัพท์/สแลง
    Explain,
    /// กด F6: ตรวจกรอบจับภาพ (บันทึกภาพ + แสดงข้อความที่ OCR อ่านได้)
    Debug,
}

struct Worker {
    cfg: Config,
    ocr: Ocr,
    tr: Translator,
    ov: usize,
    cache: HashMap<(bool, String), String>,
    history: VecDeque<String>,
    /// ข้อความ (แบบย่อ) ที่กำลังแสดงคำแปลอยู่
    last_key: String,
    last_api: Option<Instant>,
    backoff_until: Option<Instant>,
    /// ตัวอย่างความสว่างของกรอบรอบก่อน ไว้เทียบว่าเปลี่ยนไหม
    prev_sig: Option<Vec<u8>>,
    dirty_since: Option<Instant>,
    last_ocr: Instant,
    /// ข้อความที่ OCR แล้วแต่ยังรอคิวเรียก API (text, key)
    pending: Option<(String, String)>,
    /// กรอบว่างติดกันกี่รอบ (กันโอเวอร์เลย์กะพริบเมื่อ OCR พลาดครั้งเดียว)
    empty_streak: u32,
}

fn norm_key(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

fn alnum_count(s: &str) -> usize {
    s.chars().filter(|c| c.is_alphanumeric()).count()
}

/// ข้อความสองอันเหมือนกันไหม (ยอมให้ OCR อ่านเพี้ยนไม่กี่ตัวอักษร จะได้ไม่แปลซ้ำโดยไม่จำเป็น)
fn similar(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let ca: Vec<char> = a.chars().collect();
    let cb: Vec<char> = b.chars().collect();
    if ca.len() < 8 || cb.len() < 8 {
        return false; // ข้อความสั้น: ต้องตรงกันเป๊ะ
    }
    let mut counts: HashMap<(char, char), i32> = HashMap::new();
    for w in ca.windows(2) {
        *counts.entry((w[0], w[1])).or_insert(0) += 1;
    }
    let mut matches = 0usize;
    for w in cb.windows(2) {
        if let Some(c) = counts.get_mut(&(w[0], w[1])) {
            if *c > 0 {
                *c -= 1;
                matches += 1;
            }
        }
    }
    let total = (ca.len() - 1) + (cb.len() - 1);
    (2 * matches) as f64 / total as f64 >= 0.9
}

pub fn run(cfg: Config, rx: Receiver<Cmd>, ov: usize) {
    trace::log("worker: start");
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    trace::log("worker: creating OCR engine");
    let ocr = match Ocr::new() {
        Ok(o) => {
            trace::log("worker: OCR engine ready");
            o
        }
        Err(e) => {
            trace::log(&format!("worker: OCR init failed: {e}"));
            eprintln!("[ERROR] OCR init failed: {e}");
            overlay::show_text(ov, &format!("[!] {e}"));
            return;
        }
    };
    let auto = cfg.is_auto();
    let poll = Duration::from_millis(cfg.poll_ms);
    let mut w = Worker {
        tr: Translator::new(&cfg),
        cfg,
        ocr,
        ov,
        cache: HashMap::new(),
        history: VecDeque::new(),
        last_key: String::new(),
        last_api: None,
        backoff_until: None,
        prev_sig: None,
        dirty_since: None,
        last_ocr: Instant::now(),
        pending: None,
        empty_streak: 0,
    };

    loop {
        if auto {
            match rx.recv_timeout(poll) {
                Ok(cmd) => w.handle(cmd),
                Err(RecvTimeoutError::Timeout) => w.tick(),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            // โหมด hotkey: หลับรอคำสั่ง ไม่กิน CPU เลย
            match rx.recv() {
                Ok(cmd) => w.handle(cmd),
                Err(_) => break,
            }
        }
    }
}

impl Worker {
    fn say(&self, text: &str) {
        println!("[show] {text}");
        overlay::show_text(self.ov, text);
    }

    fn grab(&self, scale: i32) -> Result<Frame, String> {
        let (x, y, w, h) = region::get();
        capture::capture(x, y, w, h, scale)
    }

    fn handle(&mut self, cmd: Cmd) {
        if region::is_selecting() {
            return; // กำลังลากเลือกกรอบอยู่ ไม่ต้องทำอะไร
        }
        match cmd {
            Cmd::Translate => self.process(true, false, true),
            Cmd::Explain => self.process(true, true, true),
            Cmd::Debug => self.debug(),
        }
    }

    /// API พร้อมรับคำขอใหม่ไหม (เว้นระยะกันโควตาฟรีเต็ม)
    fn api_ready(&self) -> bool {
        let now = Instant::now();
        if self.backoff_until.map_or(false, |t| now < t) {
            return false;
        }
        self.last_api
            .map_or(true, |t| t.elapsed() >= Duration::from_millis(self.cfg.min_api_interval_ms))
    }

    /// OCR กรอบปัจจุบัน ถ้าอ่านไม่ได้ข้อความและ `fallback` เปิดอยู่ จะลองปรับภาพ (คอนทราสต์ / กลับสี) แล้วอ่านซ้ำ
    fn ocr_text(&mut self, fallback: bool) -> Result<String, String> {
        let mut frame = self.grab(self.cfg.ocr_scale as i32)?;
        let mut text = self.ocr.read(&frame)?;
        if fallback && alnum_count(&text) < 3 {
            capture::enhance(&mut frame);
            let t2 = self.ocr.read(&frame)?;
            if alnum_count(&t2) > alnum_count(&text) {
                text = t2;
            }
            if alnum_count(&text) < 3 {
                capture::invert(&mut frame);
                let t3 = self.ocr.read(&frame)?;
                if alnum_count(&t3) > alnum_count(&text) {
                    text = t3;
                }
            }
        }
        Ok(text)
    }

    fn debug(&mut self) {
        let scale = self.cfg.ocr_scale as i32;
        match self.grab(scale) {
            Err(e) => self.say(&format!("[!] {e}")),
            Ok(f) => {
                let mut path = crate::config::config_path();
                path.set_file_name("debug_capture.bmp");
                let saved = capture::save_bmp(&f, &path).is_ok();
                let text = self.ocr_text(true).unwrap_or_else(|e| format!("(OCR error: {e})"));
                let msg = format!(
                    "[ตรวจกรอบ] OCR อ่านได้: {}\n{}",
                    if text.is_empty() { "(ไม่พบข้อความ)" } else { text.as_str() },
                    if saved { "บันทึกภาพที่จับได้ไว้ที่ debug_capture.bmp ข้างไฟล์โปรแกรม" } else { "" }
                );
                self.say(&msg);
            }
        }
    }

    /// โหมด auto ทำงานทุก poll_ms: เทียบภาพแบบเบา ๆ → ภาพเปลี่ยนแล้วนิ่ง → OCR → แปล
    fn tick(&mut self) {
        if region::is_selecting() {
            return;
        }
        let frame = match self.grab(1) {
            Ok(f) => f,
            Err(_) => return,
        };
        let sig = capture::luma_sample(&frame);
        let changed = match &self.prev_sig {
            Some(prev) if prev.len() == sig.len() => {
                let diff = sig
                    .iter()
                    .zip(prev.iter())
                    .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 40)
                    .count();
                // เปลี่ยนอย่างน้อย 12 จุด และราว 0.3% ของกรอบ (ตัวอักษรเปลี่ยนจะทำให้พิกเซลเปลี่ยนมากกว่านี้เสมอ)
                diff >= 12 && diff * 1000 >= sig.len() * 3
            }
            _ => true,
        };
        self.prev_sig = Some(sig);

        let now = Instant::now();
        // Some(true) = OCR เพราะภาพเปลี่ยน (ลองปรับภาพซ้ำได้) | Some(false) = OCR ตามรอบตรวจซ้ำ
        let mut ocr_now: Option<bool> = None;
        if changed {
            let since = *self.dirty_since.get_or_insert(now);
            // ฉากหลังขยับตลอด: ถ้าไม่นิ่งเกิน 4 วินาที ก็อ่านเลยไม่ต้องรอ
            if now.duration_since(since) > Duration::from_secs(4) {
                self.dirty_since = Some(now);
                ocr_now = Some(true);
            }
        } else if self.dirty_since.take().is_some() {
            ocr_now = Some(true); // เปลี่ยนแล้วนิ่งแล้ว
        } else if now.duration_since(self.last_ocr) >= Duration::from_millis(self.cfg.auto_recheck_ms) {
            ocr_now = Some(false); // ไม่เห็นว่าเปลี่ยน แต่ครบรอบตรวจซ้ำ
        }

        if let Some(fallback) = ocr_now {
            self.last_ocr = now;
            self.process(false, false, fallback);
        } else if !changed && self.pending.is_some() && self.api_ready() {
            // มีข้อความที่ OCR ไว้แล้วรอคิว API: แปลเลย ไม่ต้อง OCR ซ้ำ
            if let Some((text, key)) = self.pending.take() {
                self.translate_and_show(text, key, false, false);
            }
        }
    }

    fn process(&mut self, manual: bool, explain: bool, fallback: bool) {
        match self.ocr_text(fallback) {
            Ok(text) => self.handle_text(text, manual, explain),
            Err(e) => {
                trace::log(&format!("worker: OCR error: {e}"));
                if manual {
                    self.say(&format!("[!] {e}"));
                }
            }
        }
    }

    fn handle_text(&mut self, text: String, manual: bool, explain: bool) {
        let key = norm_key(&text);
        if key.chars().count() < 3 {
            if manual {
                self.say("ไม่พบข้อความภาษาอังกฤษในกรอบ (กด F6 เพื่อตรวจกรอบจับภาพ หรือ F5 เพื่อเลือกกรอบใหม่)");
            } else {
                self.empty_streak += 1;
                // กรอบว่างติดกัน 2 รอบถึงซ่อนคำแปล (กันกะพริบเมื่อ OCR พลาดครั้งเดียว)
                if self.empty_streak >= 2 {
                    overlay::hide(self.ov);
                    self.last_key.clear();
                    self.pending = None;
                }
            }
            return;
        }
        self.empty_streak = 0;

        if !manual && similar(&key, &self.last_key) {
            self.pending = None; // ข้อความเดิมที่แสดงอยู่แล้ว
            return;
        }
        if let Some(cached) = self.cache.get(&(explain, key.clone())).cloned() {
            self.last_key = key;
            self.pending = None;
            self.say(&cached);
            return;
        }
        if !manual && !self.api_ready() {
            self.pending = Some((text, key)); // รอคิว แล้วค่อยแปลโดยไม่ต้อง OCR ซ้ำ
            return;
        }
        if manual {
            self.say("กำลังแปล…");
        }
        println!("[ocr] {text}");
        self.translate_and_show(text, key, explain, manual);
    }

    fn translate_and_show(&mut self, text: String, key: String, explain: bool, manual: bool) {
        self.last_api = Some(Instant::now());
        let hist: Vec<String> = self.history.iter().cloned().collect();
        match self.tr.translate(&text, &hist, explain) {
            Ok(out) => {
                if self.cache.len() > 500 {
                    self.cache.clear();
                }
                self.cache.insert((explain, key.clone()), out.clone());
                self.history.push_back(text);
                while self.history.len() > 3 {
                    self.history.pop_front();
                }
                self.last_key = key;
                self.pending = None;
                self.say(&out);
            }
            Err(TrError::RateLimited) => {
                self.backoff_until = Some(Instant::now() + Duration::from_secs(15));
                if !manual {
                    self.pending = Some((text, key)); // ลองใหม่เมื่อพ้นช่วงรอ
                }
                self.say("[!] โควตาฟรีของ Gemini เต็มชั่วคราว รอสักครู่แล้วลองใหม่");
            }
            Err(TrError::Other(m)) => {
                self.backoff_until = Some(Instant::now() + Duration::from_secs(10));
                if !manual {
                    self.pending = Some((text, key));
                }
                self.say(&format!("[!] {m}"));
            }
        }
    }
}
