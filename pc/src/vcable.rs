//! Integrated virtual microphone.
//!
//! ChudMic installs VB-CABLE (by VB-Audio, https://vb-cable.com, donationware;
//! bundling is permitted by VB-Audio's licensing terms) and renames its two
//! devices so Windows and apps show them as "ChudMic":
//!   recording device  "CABLE Output" -> "ChudMic"       (pick this as the mic)
//!   playback device   "CABLE Input"  -> "ChudMic Feed"  (ChudMic plays into this)
//!
//! Installing a driver and renaming devices need admin rights, so the GUI
//! re-launches ChudMic.exe elevated with `--setup <dir>` or `--rename`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const MIC_NAME: &str = "ChudMic";
pub const FEED_NAME: &str = "ChudMic Feed";
pub const PACK_URL: &str = "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack46.zip";
pub const DONATE_URL: &str = "https://vb-audio.com/Cable/";

/// The official VB-CABLE driver pack, embedded at build time (see build.rs).
/// Empty if it wasn't available when building; then it is downloaded instead.
static PACK: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/vbcable.zip"));

// Exit codes of the elevated helper.
pub const OK: i32 = 0;
pub const E_NOT_FOUND: i32 = 11;
pub const E_RENAME: i32 = 12;
pub const E_PACK: i32 = 13;
pub const E_CANCELLED: i32 = 1223;

pub fn describe_exit(code: i32) -> Result<String, String> {
    match code {
        OK => Ok("The ChudMic microphone is ready.".into()),
        E_NOT_FOUND => Err(
            "The driver was installed, but Windows hasn't shown the device yet. \
                            Restart your PC, then open ChudMic again."
                .into(),
        ),
        E_RENAME => Err("The virtual mic works, but renaming it to \"ChudMic\" failed.".into()),
        E_PACK => Err("The VB-CABLE driver pack is missing or damaged.".into()),
        E_CANCELLED => Err("Setup cancelled (the Windows admin prompt was declined).".into()),
        c => Err(format!("Setup failed (code {c}).")),
    }
}

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Unpack the driver pack into %TEMP%\ChudMicSetup and return that folder.
pub fn prepare_pack() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("ChudMicSetup");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let zip = dir.join("vbcable.zip");
    if PACK.is_empty() {
        let ok = hidden(
            Command::new("curl.exe")
                .args(["-L", "--fail", "-s", "-o"])
                .arg(&zip)
                .arg(PACK_URL),
        )
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
        if !ok {
            return Err("Couldn't download VB-CABLE. Check your internet connection.".into());
        }
    } else {
        std::fs::write(&zip, PACK).map_err(|e| e.to_string())?;
    }
    let ok = hidden(
        Command::new("tar.exe")
            .arg("-xf")
            .arg(&zip)
            .arg("-C")
            .arg(&dir),
    )
    .status()
    .map(|s| s.success())
    .unwrap_or(false);
    if !ok || find_file(&dir, "VBCABLE_Setup_x64.exe").is_none() {
        return Err("Couldn't unpack the VB-CABLE driver pack.".into());
    }
    Ok(dir)
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)) {
            return Some(p);
        }
    }
    None
}

/// Entry point when ChudMic.exe runs elevated as the setup helper.
pub fn elevated_main(args: &[String]) -> i32 {
    match args.get(1).map(String::as_str) {
        Some("--setup") => {
            // Already installed (e.g. a second click)? Then only fix the names.
            if let Ok(n) = rename_endpoints() {
                if n >= 2 {
                    return OK;
                }
            }
            let Some(setup) = args
                .get(2)
                .and_then(|d| find_file(Path::new(d), "VBCABLE_Setup_x64.exe"))
            else {
                return E_PACK;
            };
            // -i = install, -h = hidden (silent). The installer's exit code isn't
            // reliable, so we check for the devices afterwards instead.
            let _ = hidden(Command::new(&setup).args(["-i", "-h"]))
                .current_dir(setup.parent().unwrap_or(Path::new(".")))
                .status();
            // Windows can take a few seconds to create the new devices.
            for _ in 0..40 {
                if let Ok(n) = rename_endpoints() {
                    if n >= 2 {
                        return OK;
                    }
                }
                std::thread::sleep(Duration::from_millis(750));
            }
            E_NOT_FOUND
        }
        Some("--rename") => match rename_endpoints() {
            Ok(n) if n > 0 => OK,
            Ok(_) => E_NOT_FOUND,
            Err(_) => E_RENAME,
        },
        _ => 2,
    }
}

/// Rename VB-CABLE's endpoints to ChudMic / ChudMic Feed. Needs admin.
/// Returns how many endpoints were renamed.
#[cfg(windows)]
pub fn rename_endpoints() -> Result<usize, String> {
    use windows::core::PROPVARIANT;
    use windows::Win32::Devices::FunctionDiscovery::{
        PKEY_DeviceInterface_FriendlyName, PKEY_Device_DeviceDesc,
    };
    use windows::Win32::Media::Audio::{
        eCapture, eRender, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::StructuredStorage::{
        PropVariantChangeType, PROPVAR_CHANGE_FLAGS,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READWRITE,
    };
    use windows::Win32::System::Variant::VT_LPWSTR;

    let err = |e: windows::core::Error| e.to_string();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err)?;
        let mut renamed = 0;
        for (flow, new_name) in [(eCapture, MIC_NAME), (eRender, FEED_NAME)] {
            let devices = enumerator
                .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
                .map_err(err)?;
            for i in 0..devices.GetCount().map_err(err)? {
                let device = devices.Item(i).map_err(err)?;
                let store = device.OpenPropertyStore(STGM_READWRITE).map_err(err)?;
                let iface = store
                    .GetValue(&PKEY_DeviceInterface_FriendlyName)
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                if !iface.contains("VB-Audio Virtual Cable") {
                    continue;
                }
                // Windows stores device names as VT_LPWSTR. PROPVARIANT::from(&str)
                // makes a VT_BSTR, which audio libraries (cpal) can't read back,
                // so convert it to the type Windows expects.
                let as_bstr = PROPVARIANT::from(new_name);
                let mut value = PROPVARIANT::default();
                PropVariantChangeType(&mut value, &as_bstr, PROPVAR_CHANGE_FLAGS(0), VT_LPWSTR)
                    .map_err(err)?;
                store
                    .SetValue(&PKEY_Device_DeviceDesc, &value)
                    .map_err(err)?;
                store.Commit().map_err(err)?;
                renamed += 1;
            }
        }
        Ok(renamed)
    }
}

#[cfg(not(windows))]
pub fn rename_endpoints() -> Result<usize, String> {
    Err("Windows only".into())
}

/// Run ChudMic.exe elevated (UAC prompt) with `args`, wait, return its exit code.
#[cfg(windows)]
pub fn run_elevated(args: &str) -> i32 {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let Ok(exe) = std::env::current_exe() else {
        return -1;
    };
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let file = wide(exe.as_os_str());
    let params = wide(std::ffi::OsStr::new(args));

    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if ShellExecuteExW(&mut info).is_err() {
            return E_CANCELLED;
        }
        if info.hProcess.is_invalid() {
            return -1;
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        code as i32
    }
}

#[cfg(not(windows))]
pub fn run_elevated(_args: &str) -> i32 {
    -1
}

/// Open a URL or ms-settings page with the default handler.
pub fn open(target: &str) {
    let _ = hidden(Command::new("cmd").args(["/C", "start", "", target])).spawn();
}
