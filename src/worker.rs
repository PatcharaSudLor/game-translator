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
    last_key: String,
    last_api: Option<Instant>,
    backoff_until: Option<Instant>,
    prev_grid: Option<(usize, usize, Vec<u8>)>,
    dirty_since: Option<Instant>,
    need_retry: bool,
}

fn norm_key(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
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
        prev_grid: None,
        dirty_since: None,
        need_retry: false,
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
            Cmd::Translate => self.process(true, false),
            Cmd::Explain => self.process(true, true),
            Cmd::Debug => self.debug(),
        }
    }

    fn debug(&mut self) {
        let scale = self.cfg.ocr_scale as i32;
        match self.grab(scale) {
            Err(e) => self.say(&format!("[!] {e}")),
            Ok(f) => {
                let mut path = crate::config::config_path();
                path.set_file_name("debug_capture.bmp");
                let saved = capture::save_bmp(&f, &path).is_ok();
                let text = self.ocr.read(&f).unwrap_or_else(|e| format!("(OCR ผิดพลาด: {e})"));
                let msg = format!(
                    "[ตรวจกรอบ] OCR อ่านได้: {}\n{}",
                    if text.is_empty() { "(ไม่พบข้อความ)" } else { text.as_str() },
                    if saved { "บันทึกภาพที่จับได้ไว้ที่ debug_capture.bmp ข้างไฟล์โปรแกรม" } else { "" }
                );
                self.say(&msg);
            }
        }
    }

    /// โหมด auto: เทียบภาพแบบเบา ๆ ก่อน ถ้าภาพนิ่งแล้วค่อย OCR
    fn tick(&mut self) {
        if region::is_selecting() {
            return;
        }
        let frame = match self.grab(1) {
            Ok(f) => f,
            Err(_) => return,
        };
        let grid = capture::luma_grid(&frame);
        let changed = match &self.prev_grid {
            Some((pw, ph, pg)) if *pw == grid.0 && *ph == grid.1 => {
                let diff = grid
                    .2
                    .iter()
                    .zip(pg.iter())
                    .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 40)
                    .count();
                diff * 100 >= grid.2.len() * 2 // เปลี่ยนตั้งแต่ 2% ของกรอบ
            }
            _ => true,
        };
        self.prev_grid = Some((grid.0, grid.1, grid.2));

        if changed {
            let since = *self.dirty_since.get_or_insert_with(Instant::now);
            // ฉากหลังขยับตลอดเวลา: ถ้าไม่นิ่งเกิน 4 วินาทีก็ลอง OCR เลย
            if since.elapsed() > Duration::from_secs(4) {
                self.dirty_since = Some(Instant::now());
                self.process(false, false);
            }
            return;
        }
        if self.dirty_since.is_some() || self.need_retry {
            self.dirty_since = None;
            self.process(false, false);
        }
    }

    fn process(&mut self, manual: bool, explain: bool) {
        let frame = match self.grab(self.cfg.ocr_scale as i32) {
            Ok(f) => f,
            Err(e) => {
                if manual {
                    self.say(&format!("[!] {e}"));
                }
                return;
            }
        };
        let text = match self.ocr.read(&frame) {
            Ok(t) => t,
            Err(e) => {
                if manual {
                    self.say(&format!("[!] {e}"));
                }
                return;
            }
        };
        let key = norm_key(&text);
        if key.chars().count() < 3 {
            if manual {
                self.say("ไม่พบข้อความภาษาอังกฤษในกรอบ (กด F6 เพื่อตรวจกรอบจับภาพ)");
            } else {
                // กล่องข้อความหายไป: ซ่อนคำแปล และให้ประโยคเดิมแปลใหม่ได้ถ้ากลับมา
                overlay::hide(self.ov);
                self.last_key.clear();
                self.need_retry = false;
            }
            return;
        }
        if !manual && key == self.last_key {
            self.need_retry = false;
            return;
        }
        if let Some(cached) = self.cache.get(&(explain, key.clone())).cloned() {
            self.last_key = key;
            self.need_retry = false;
            self.say(&cached);
            return;
        }

        if !manual {
            let now = Instant::now();
            let blocked = self.backoff_until.map_or(false, |t| now < t)
                || self
                    .last_api
                    .map_or(false, |t| t.elapsed() < Duration::from_millis(self.cfg.min_api_interval_ms));
            if blocked {
                self.need_retry = true; // รอบหน้าลองใหม่ ไม่ทิ้งประโยคนี้
                return;
            }
        } else {
            self.say("กำลังแปล…");
        }

        println!("[ocr] {text}");
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
                self.need_retry = false;
                self.say(&out);
            }
            Err(TrError::RateLimited) => {
                self.backoff_until = Some(Instant::now() + Duration::from_secs(30));
                self.need_retry = !manual;
                self.say("[!] โควตาฟรีของ Gemini เต็มชั่วคราว รอสักครู่แล้วลองใหม่");
            }
            Err(TrError::Other(m)) => {
                self.need_retry = false;
                self.say(&format!("[!] {m}"));
            }
        }
    }
}
