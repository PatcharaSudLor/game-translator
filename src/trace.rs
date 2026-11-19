//! บันทึกขั้นตอนการทำงานลง trace.log (ข้างไฟล์ .exe) ไว้หาสาเหตุเมื่อโปรแกรมล้ม
use std::io::Write;

fn path() -> std::path::PathBuf {
    let mut p = crate::config::config_path();
    p.set_file_name("trace.log");
    p
}

pub fn reset() {
    let _ = std::fs::remove_file(path());
}

pub fn log(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path()) {
        let _ = writeln!(f, "{msg}");
    }
}
