use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::PathBuf};

pub const DEFAULT_CONFIG: &str = r##"# ===== Game Translator: ไฟล์ตั้งค่า =====
# แก้ไขแล้วเปิดโปรแกรมใหม่ (ปิดด้วย Ctrl+Shift+Q)

# "hotkey" = กด F8 เมื่อสงสัยแล้วแปลเฉพาะตอนนั้น (ประหยัดสุด)
# "auto"   = แปลอัตโนมัติเมื่อข้อความในกรอบเปลี่ยน
mode = "hotkey"

# Gemini API key (ขอฟรีที่ https://aistudio.google.com/apikey)
# หรือจะตั้งเป็นตัวแปรสภาพแวดล้อมชื่อ GEMINI_API_KEY ก็ได้
api_key = ""

# ชื่อโมเดล (ถ้าโมเดลนี้ใช้ไม่ได้ ให้ดูชื่อรุ่น Flash-Lite ล่าสุดใน AI Studio)
model = "gemini-3.1-flash-lite"

# บอกบริบทเกมให้แปลตรงโทนขึ้น (แก้ได้ตามเกมที่เล่น)
game_context = "เกมแนวเนื้อเรื่อง (ผจญภัยแฟนตาซี/สยองขวัญ) ที่เน้นบทสนทนา"

# ความถี่ตรวจหน้าจอในโหมด auto (มิลลิวินาที) ยิ่งมากยิ่งประหยัดเครื่อง
poll_ms = 800
# ระยะห่างขั้นต่ำระหว่างการเรียก API ในโหมด auto (มิลลิวินาที) กันโควตาฟรีเต็ม
min_api_interval_ms = 4500

# ขยายภาพก่อน OCR (2 = เท่าตัว) ช่วยให้อ่านตัวอักษรเล็กได้แม่นขึ้น (ใช้ 1-3)
ocr_scale = 2

# คำแปลค้างบนจอกี่วินาที (กด F9 เพื่อซ่อนทันที)
overlay_seconds = 12
font_name = "Leelawadee UI"
font_size = 28
# ความทึบของแผ่นคำแปล 60-255
overlay_opacity = 235

# กรอบบทสนทนาที่จะจับภาพ เป็นสัดส่วนของจอ (0.0 - 1.0)
# x,y = มุมซ้ายบน  w,h = กว้าง,สูง  (ค่าเริ่มต้น = แถบล่างของจอ)
# กด F6 เพื่อดูว่ากรอบจับภาพได้อะไร (บันทึกเป็น debug_capture.bmp)
[region]
x = 0.10
y = 0.72
w = 0.80
h = 0.24

# ศัพท์เฉพาะ/ชื่อตัวละครที่อยากให้แปลคงที่ (เพิ่มได้เรื่อย ๆ)
[glossary]
# "Aldric" = "อัลดริก"
# "Mana" = "มานา"
"##;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Default for Region {
    fn default() -> Self {
        Region { x: 0.10, y: 0.72, w: 0.80, h: 0.24 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub mode: String,
    pub api_key: String,
    pub model: String,
    pub game_context: String,
    pub poll_ms: u64,
    pub min_api_interval_ms: u64,
    pub ocr_scale: u32,
    pub overlay_seconds: u32,
    pub font_name: String,
    pub font_size: i32,
    pub overlay_opacity: u32,
    pub region: Region,
    pub glossary: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mode: "hotkey".into(),
            api_key: String::new(),
            model: "gemini-3.1-flash-lite".into(),
            game_context: String::new(),
            poll_ms: 800,
            min_api_interval_ms: 4500,
            ocr_scale: 2,
            overlay_seconds: 12,
            font_name: "Leelawadee UI".into(),
            font_size: 28,
            overlay_opacity: 235,
            region: Region::default(),
            glossary: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn is_auto(&self) -> bool {
        self.mode.trim().eq_ignore_ascii_case("auto")
    }
}

pub fn config_path() -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    p.pop();
    p.push("config.toml");
    p
}

pub fn load() -> Result<Config, String> {
    let path = config_path();
    if !path.exists() {
        fs::write(&path, DEFAULT_CONFIG)
            .map_err(|e| format!("สร้าง config.toml ไม่ได้: {e}"))?;
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("อ่าน config.toml ไม่ได้: {e}"))?;
    let mut cfg: Config =
        toml::from_str(&text).map_err(|e| format!("config.toml รูปแบบไม่ถูกต้อง: {e}"))?;

    if cfg.api_key.trim().is_empty() {
        if let Ok(k) = std::env::var("GEMINI_API_KEY") {
            cfg.api_key = k;
        }
    }
    cfg.api_key = cfg.api_key.trim().to_string();
    cfg.ocr_scale = cfg.ocr_scale.clamp(1, 3);
    cfg.poll_ms = cfg.poll_ms.clamp(300, 10_000);
    cfg.overlay_seconds = cfg.overlay_seconds.clamp(2, 120);
    cfg.overlay_opacity = cfg.overlay_opacity.clamp(60, 255);
    cfg.font_size = cfg.font_size.clamp(12, 80);
    let r = &mut cfg.region;
    r.x = r.x.clamp(0.0, 0.99);
    r.y = r.y.clamp(0.0, 0.99);
    r.w = r.w.clamp(0.02, 1.0 - r.x);
    r.h = r.h.clamp(0.02, 1.0 - r.y);
    Ok(cfg)
}

/// เขียนค่ากรอบใหม่ลงใน [region] ของ config.toml โดยคงคอมเมนต์และค่าอื่นไว้
pub fn save_region(x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
    let path = config_path();
    let text = fs::read_to_string(&path).map_err(|e| format!("อ่าน config.toml ไม่ได้: {e}"))?;
    let new_lines = [
        format!("x = {:.4}", x),
        format!("y = {:.4}", y),
        format!("w = {:.4}", w),
        format!("h = {:.4}", h),
    ];

    let mut out: Vec<String> = Vec::new();
    let mut in_region = false;
    let mut found = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_region = t.starts_with("[region]");
            out.push(line.to_string());
            if in_region {
                found = true;
                out.extend(new_lines.iter().cloned());
            }
            continue;
        }
        if in_region && t.contains('=') {
            let key = t.split('=').next().unwrap_or("").trim();
            if matches!(key, "x" | "y" | "w" | "h") {
                continue; // ค่าเก่า ถูกแทนด้วยค่าใหม่ที่ใส่ไว้หลังหัวข้อแล้ว
            }
        }
        out.push(line.to_string());
    }
    if !found {
        out.push(String::new());
        out.push("[region]".to_string());
        out.extend(new_lines.iter().cloned());
    }
    let mut result = out.join("\n");
    result.push('\n');
    fs::write(&path, result).map_err(|e| format!("เขียน config.toml ไม่ได้: {e}"))
}
