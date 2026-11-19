// ฝังไอคอนและข้อมูลเวอร์ชันเข้าไฟล์ .exe (ทำงานเฉพาะตอน build บน Windows)
fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=icon.ico");
    embed_resource::compile("app.rc", embed_resource::NONE);
}
