// No console window in release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod audio;
mod net;
mod pitch;
mod proto;

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};

use audio::AudioShared;
use net::Net;

const GREEN: Color32 = Color32::from_rgb(80, 200, 120);
const RED: Color32 = Color32::from_rgb(200, 60, 60);
const ORANGE: Color32 = Color32::from_rgb(230, 160, 50);

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ChudMic")
            .with_inner_size([380.0, 660.0])
            .with_min_inner_size([320.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ChudMic",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

// ---------- settings (volume + output device), stored in %APPDATA%\ChudMic\settings.txt ----------

struct Settings {
    volume: f32,
    pitch: f32,
    device: Option<String>,
}

fn settings_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("ChudMic").join("settings.txt")
}

fn load_settings() -> Settings {
    let mut s = Settings {
        volume: 100.0,
        pitch: 0.0,
        device: None,
    };
    if let Ok(text) = std::fs::read_to_string(settings_path()) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "volume" => s.volume = v.trim().parse().unwrap_or(100.0f32).clamp(0.0, 200.0),
                    "pitch" => {
                        s.pitch = (v.trim().parse().unwrap_or(0.0f32) * 2.0)
                            .round()
                            .clamp(-12.0, 12.0)
                            / 2.0
                    }
                    "device" if !v.trim().is_empty() => s.device = Some(v.trim().to_string()),
                    _ => {}
                }
            }
        }
    }
    s
}

fn save_settings(s: &Settings) {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = format!(
        "volume={}\npitch={}\ndevice={}\n",
        s.volume.round(),
        s.pitch,
        s.device.clone().unwrap_or_default()
    );
    let _ = std::fs::write(path, text);
}

// ---------- QR code ----------

fn qr_image(text: &str) -> Option<egui::ColorImage> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let w = code.width();
    let quiet = 4;
    let size = w + quiet * 2;
    let colors = code.to_colors();
    let mut img = egui::ColorImage::new([size, size], Color32::WHITE);
    for y in 0..w {
        for x in 0..w {
            if matches!(colors[y * w + x], qrcode::Color::Dark) {
                img.pixels[(y + quiet) * size + x + quiet] = Color32::BLACK;
            }
        }
    }
    Some(img)
}

// ---------- app ----------

struct App {
    audio: Arc<AudioShared>,
    net: Net,
    stream: Option<cpal::Stream>,
    out_name: String,
    devices: Vec<String>,
    audio_err: Option<String>,
    settings: Settings,
    save_at: Option<Instant>,
    muted: bool,
    level: f32,
    ip: Option<Ipv4Addr>,
    ip_checked: Option<Instant>,
    qr: Option<egui::TextureHandle>,
}

impl App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let settings = load_settings();
        let audio = AudioShared::new(settings.volume / 100.0);
        let net = Net::start(audio.clone());
        let mut app = App {
            audio,
            net,
            stream: None,
            out_name: String::new(),
            devices: audio::output_device_names(),
            audio_err: None,
            settings,
            save_at: None,
            muted: false,
            level: 0.0,
            ip: None,
            ip_checked: None,
            qr: None,
        };
        let wanted = app.settings.device.clone();
        app.open_output(wanted.as_deref());
        app
    }

    fn open_output(&mut self, wanted: Option<&str>) {
        self.stream = None; // stop the old stream first
        match audio::start_output(wanted, self.audio.clone()) {
            Ok((stream, name)) => {
                self.stream = Some(stream);
                self.out_name = name;
                self.audio_err = None;
            }
            Err(e) => {
                self.out_name.clear();
                self.audio_err = Some(e);
            }
        }
    }

    fn refresh_ip(&mut self, ctx: &egui::Context) {
        let due = self
            .ip_checked
            .map_or(true, |t| t.elapsed() > Duration::from_secs(5));
        if !due {
            return;
        }
        self.ip_checked = Some(Instant::now());
        let ip = net::local_ipv4();
        if ip != self.ip || self.qr.is_none() {
            self.ip = ip;
            self.qr = ip.and_then(|ip| {
                let text = format!(
                    "CHUDMIC|1|{}|{}|{}",
                    ip,
                    proto::SESSION_PORT,
                    self.net.pc_name
                );
                qr_image(&text)
                    .map(|img| ctx.load_texture("qr", img, egui::TextureOptions::NEAREST))
            });
        }
    }
}

fn big_button(ui: &mut egui::Ui, text: &str, fill: Option<Color32>) -> bool {
    let mut b = egui::Button::new(RichText::new(text).size(18.0));
    if let Some(c) = fill {
        b = b.fill(c);
    }
    ui.add_sized([ui.available_width(), 40.0], b).clicked()
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(50));
        self.refresh_ip(ctx);

        // Push GUI controls to the audio thread, read the level meter.
        {
            let mut b = self.audio.lock();
            b.volume = self.settings.volume / 100.0;
            b.muted = self.muted;
            b.pitch = self.settings.pitch;
            self.level = b.level.max(self.level * 0.85);
            b.level = 0.0;
        }
        let (session, phones) = {
            let mut st = self.net.state();
            st.pc_muted = self.muted;
            (st.session.clone(), st.phones())
        };

        if self.save_at.is_some_and(|t| Instant::now() >= t) {
            self.save_at = None;
            save_settings(&self.settings);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("ChudMic");
                ui.weak("Your phone as a Wi-Fi microphone");
                ui.add_space(6.0);
                for e in &self.net.errors {
                    ui.colored_label(ORANGE, e);
                }
                ui.separator();

                // ----- connection -----
                match &session {
                    Some(s) => {
                        ui.label(RichText::new(format!("Connected to {}", s.name)).color(GREEN).size(16.0));
                        ui.weak(s.addr.ip().to_string());
                        if s.phone_muted {
                            ui.colored_label(ORANGE, "The phone has muted its mic");
                        }
                        ui.add_space(4.0);
                        if big_button(ui, "Disconnect", None) {
                            self.net.disconnect();
                        }
                    }
                    None => {
                        ui.label(RichText::new("Not connected").size(16.0));
                        ui.add_space(4.0);
                        ui.label("Phones on this Wi-Fi:");
                        if phones.is_empty() {
                            ui.weak("None found yet. Open ChudMic on your phone.");
                        }
                        for p in &phones {
                            ui.horizontal(|ui| {
                                if ui.button("Connect").clicked() {
                                    self.net.request_connect(p.ip);
                                }
                                ui.label(&p.name);
                                ui.weak(p.ip.to_string());
                            });
                        }
                    }
                }
                ui.separator();

                // ----- mute + volume -----
                let (label, fill) = if self.muted { ("Unmute", Some(RED)) } else { ("Mute", None) };
                if big_button(ui, label, fill) {
                    self.muted = !self.muted;
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Volume");
                    let r = ui.add(
                        egui::Slider::new(&mut self.settings.volume, 0.0..=200.0)
                            .suffix("%")
                            .integer(),
                    );
                    if r.changed() {
                        self.save_at = Some(Instant::now() + Duration::from_millis(600));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Pitch");
                    let r = ui
                        .add(
                            egui::Slider::new(&mut self.settings.pitch, -6.0..=6.0)
                                .step_by(0.5)
                                .custom_formatter(|v, _| {
                                    if v == 0.0 {
                                        "0 (off)".to_string()
                                    } else {
                                        format!("{v:+} st")
                                    }
                                }),
                        )
                        .on_hover_text("Shift your voice up or down. Adds about 50 ms of delay when not 0.");
                    if r.changed() {
                        self.save_at = Some(Instant::now() + Duration::from_millis(600));
                    }
                    if self.settings.pitch != 0.0 && ui.small_button("Reset").clicked() {
                        self.settings.pitch = 0.0;
                        self.save_at = Some(Instant::now() + Duration::from_millis(600));
                    }
                });
                ui.add(egui::ProgressBar::new(self.level.sqrt().min(1.0)));
                ui.separator();

                // ----- output device -----
                ui.label("Send audio to:");
                let mut chosen: Option<String> = None;
                ui.horizontal(|ui| {
                    let current = if self.out_name.is_empty() { "(none)" } else { self.out_name.as_str() };
                    egui::ComboBox::new("output_device", "")
                        .width(ui.available_width() - 70.0)
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for d in &self.devices {
                                if ui.selectable_label(*d == self.out_name, d).clicked() {
                                    chosen = Some(d.clone());
                                }
                            }
                        });
                    if ui.button("Refresh").clicked() {
                        self.devices = audio::output_device_names();
                    }
                });
                if let Some(d) = chosen {
                    self.open_output(Some(&d));
                    self.settings.device = Some(d);
                    save_settings(&self.settings);
                }
                if let Some(e) = &self.audio_err {
                    ui.colored_label(RED, e);
                } else if audio::is_virtual_cable(&self.out_name) {
                    ui.weak("In Discord, OBS, games etc. choose \"CABLE Output\" as your microphone.");
                } else {
                    ui.colored_label(
                        ORANGE,
                        "VB-Cable not selected: you'll hear the phone on this device, but apps won't \
                         see it as a mic. Install VB-Cable and pick \"CABLE Input\".",
                    );
                }

                // ----- QR -----
                if session.is_none() {
                    ui.separator();
                    ui.label("Phone not listed? Tap \"Scan QR\" in the phone app:");
                    match (&self.qr, self.ip) {
                        (Some(tex), Some(ip)) => {
                            ui.vertical_centered(|ui| {
                                ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                    tex.id(),
                                    egui::vec2(200.0, 200.0),
                                )));
                                ui.weak(format!("{}:{}", ip, proto::SESSION_PORT));
                            });
                        }
                        _ => {
                            ui.colored_label(ORANGE, "No network connection found.");
                        }
                    }
                }
            });
        });
    }
}
