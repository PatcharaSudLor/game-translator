use crate::capture::Frame;
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

pub struct Ocr {
    engine: OcrEngine,
}

fn e<T: std::fmt::Display>(ctx: &str, err: T) -> String {
    format!("{ctx}: {err}")
}

impl Ocr {
    pub fn new() -> Result<Self, String> {
        let lang = Language::CreateLanguage(&HSTRING::from("en-US"))
            .map_err(|x| e("สร้างภาษา en-US", x))?;
        let supported = OcrEngine::IsLanguageSupported(&lang).unwrap_or(false);
        let created = if supported {
            OcrEngine::TryCreateFromLanguage(&lang)
        } else {
            OcrEngine::TryCreateFromUserProfileLanguages()
        };
        let engine = created.map_err(|_| {
            "ไม่พบ OCR ภาษาอังกฤษของ Windows: ไปที่ Settings > Time & Language > Language \
             เพิ่มภาษา English (United States) แล้วรีสตาร์ทเครื่อง"
                .to_string()
        })?;
        Ok(Ocr { engine })
    }

    pub fn read(&self, f: &Frame) -> Result<String, String> {
        let max = OcrEngine::MaxImageDimension().unwrap_or(10_000) as i32;
        if f.width > max || f.height > max {
            return Err("กรอบจับภาพใหญ่เกินที่ OCR รองรับ ลดค่า ocr_scale หรือขนาดกรอบ".into());
        }
        let writer = DataWriter::new().map_err(|x| e("DataWriter", x))?;
        writer.WriteBytes(&f.bgra).map_err(|x| e("WriteBytes", x))?;
        let buffer = writer.DetachBuffer().map_err(|x| e("DetachBuffer", x))?;
        let bmp = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            f.width,
            f.height,
        )
        .map_err(|x| e("SoftwareBitmap", x))?;
        let result = self
            .engine
            .RecognizeAsync(&bmp)
            .map_err(|x| e("RecognizeAsync", x))?
            .get()
            .map_err(|x| e("OCR", x))?;

        let mut parts: Vec<String> = Vec::new();
        for line in result.Lines().map_err(|x| e("Lines", x))? {
            if let Ok(t) = line.Text() {
                parts.push(t.to_string());
            }
        }
        // รวมบรรทัดเป็นประโยคเดียว และจัดช่องว่างให้เรียบร้อย
        let joined = parts.join(" ");
        Ok(joined.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}
