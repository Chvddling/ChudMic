// Embeds the official VB-CABLE driver pack into ChudMic.exe if it is present at
// pc/vendor/VBCABLE_Driver_Pack.zip (the CI and build script download it there).
// Without it, ChudMic downloads the pack from vb-audio.com when setting up.
use std::path::{Path, PathBuf};

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("vbcable.zip");
    let src = Path::new("vendor/VBCABLE_Driver_Pack.zip");
    println!("cargo:rerun-if-changed=vendor/VBCABLE_Driver_Pack.zip");
    if src.exists() {
        std::fs::copy(src, &out).expect("copy VB-CABLE pack");
    } else {
        std::fs::write(&out, b"").unwrap();
        println!("cargo:warning=VB-CABLE driver pack not embedded; ChudMic will download it during setup.");
    }
}
