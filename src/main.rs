mod capture;
mod config;
mod ocr;
mod overlay;
mod region;
mod select;
mod trace;
mod translate;
mod worker;

use std::sync::mpsc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, WPARAM};
use windows::Win32::System::Console::{GetConsoleWindow, SetConsoleOutputCP};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use worker::Cmd;

const HK_TRANSLATE: i32 = 1;
const HK_EXPLAIN: i32 = 2;
const HK_HIDE: i32 = 3;
const HK_DEBUG: i32 = 4;
const HK_SELECT: i32 = 5;
const HK_QUIT: i32 = 9;

fn pause() {
    println!("\nPress Enter to close this window...");
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
}

fn fail(msg: &str) -> ! {
    eprintln!("\n[ERROR] {msg}");
    pause();
    std::process::exit(1);
}

/// ตั้งไอคอนของหน้าต่างคอนโซลให้ตรงกับไอคอนโปรแกรม (ที่ฝังไว้ในไฟล์ .exe เป็นรหัส 1)
unsafe fn set_console_icon() {
    let con = GetConsoleWindow();
    if con.0.is_null() {
        return;
    }
    let Ok(module) = GetModuleHandleW(None) else { return };
    let hinst: HINSTANCE = module.into();
    if let Ok(icon) = LoadIconW(hinst, PCWSTR(1 as *const u16)) {
        SendMessageW(con, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(icon.0 as isize));
        SendMessageW(con, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(icon.0 as isize));
    }
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("Game Translator crashed: {info}");
        eprintln!("\n{msg}");
        let mut p = config::config_path();
        p.set_file_name("crash.log");
        let _ = std::fs::write(&p, &msg);
        eprintln!("Details were saved to {}", p.display());
        eprintln!("Press Enter to close...");
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
    }));
}

fn main() {
    trace::reset();
    trace::log("main: start");
    install_panic_hook();
    unsafe {
        let _ = SetConsoleOutputCP(65001);
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        set_console_icon();
    }
    println!("=== Game Translator (EN -> TH) ===");

    let cfg = config::load().unwrap_or_else(|e| fail(&e));
    if cfg.api_key.is_empty() {
        println!("\nNo Gemini API key yet.");
        println!("1) Get a free key at https://aistudio.google.com/apikey");
        println!("2) Open {} and put it in api_key = \"...\"", config::config_path().display());
        println!("3) Start the program again");
        pause();
        return;
    }

    trace::log("main: config loaded");
    let (sw, sh) = capture::screen_size();
    let rect = (
        (sw as f64 * cfg.region.x) as i32,
        (sh as f64 * cfg.region.y) as i32,
        ((sw as f64 * cfg.region.w) as i32).max(16),
        ((sh as f64 * cfg.region.h) as i32).max(16),
    );

    region::set(rect);
    let hwnd = overlay::create(overlay::Layout {
        screen_w: sw,
        screen_h: sh,
        font_name: cfg.font_name.clone(),
        font_px: cfg.font_size,
        opacity: cfg.overlay_opacity as u8,
        seconds: cfg.overlay_seconds,
    })
    .unwrap_or_else(|e| fail(&format!("Cannot create overlay window: {e}")));
    let ov = overlay::to_raw(hwnd);
    trace::log("main: overlay window created");

    let (tx, rx) = mpsc::channel::<Cmd>();
    let auto = cfg.is_auto();
    {
        let cfg = cfg.clone();
        std::thread::spawn(move || worker::run(cfg, rx, ov));
    }

    unsafe {
        let keys = [
            (HK_TRANSLATE, MOD_NOREPEAT, VK_F8.0 as u32, "F8  = translate now"),
            (HK_EXPLAIN, MOD_NOREPEAT, VK_F7.0 as u32, "F7  = translate + explain slang"),
            (HK_SELECT, MOD_NOREPEAT, VK_F5.0 as u32, "F5  = drag to select text region"),
            (HK_DEBUG, MOD_NOREPEAT, VK_F6.0 as u32, "F6  = test capture region (OCR)"),
            (HK_HIDE, MOD_NOREPEAT, VK_F9.0 as u32, "F9  = hide translation"),
            (HK_QUIT, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, 'Q' as u32, "Ctrl+Shift+Q = quit"),
        ];
        println!("\nMode: {}", if auto { "auto" } else { "hotkey" });
        println!("Hotkeys:");
        for (id, m, vk, label) in keys {
            if RegisterHotKey(None, id, m, vk).is_ok() {
                println!("  {label}");
            } else {
                println!("  (could not register: {label} - another program may be using this key)");
            }
        }
        println!("\nReady. Run your game in Borderless Window mode, then use the hotkeys.\n");

        trace::log("main: hotkeys registered, entering message loop");
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == -1 {
                eprintln!("[ERROR] GetMessageW failed; the program will exit.");
                pause();
                break;
            }
            if r.0 == 0 {
                break;
            }
            if msg.message == WM_HOTKEY {
                trace::log(&format!("main: hotkey id={}", msg.wParam.0));
                match msg.wParam.0 as i32 {
                    HK_TRANSLATE => {
                        let _ = tx.send(Cmd::Translate);
                    }
                    HK_EXPLAIN => {
                        let _ = tx.send(Cmd::Explain);
                    }
                    HK_DEBUG => {
                        let _ = tx.send(Cmd::Debug);
                    }
                    HK_SELECT => select::begin(),
                    HK_HIDE => overlay::hide(ov),
                    HK_QUIT => break,
                    _ => {}
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        trace::log("main: message loop ended");
        for (id, ..) in keys {
            let _ = UnregisterHotKey(None, id);
        }
    }
}
