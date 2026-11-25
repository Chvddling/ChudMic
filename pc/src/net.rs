//! Discovery (find phones on the LAN) and the audio session with one phone.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::audio::AudioShared;
use crate::pitch::PitchShifter;
use crate::proto::*;

const SESSION_TIMEOUT: Duration = Duration::from_secs(3);
const PING_EVERY: Duration = Duration::from_millis(500);
const ANNOUNCE_EVERY: Duration = Duration::from_millis(1500);
const PHONE_EXPIRES: Duration = Duration::from_secs(6);

#[derive(Clone)]
pub struct PhoneInfo {
    pub ip: IpAddr,
    pub name: String,
    last_seen: Instant,
}

#[derive(Clone)]
pub struct Session {
    pub addr: SocketAddr,
    pub name: String,
    pub phone_muted: bool,
    last_rx: Instant,
    last_seq: Option<u32>,
}

#[derive(Default)]
pub struct NetState {
    phones: HashMap<IpAddr, PhoneInfo>,
    pub session: Option<Session>,
    pub pc_muted: bool,
    kick: bool,
}

impl NetState {
    pub fn phones(&self) -> Vec<PhoneInfo> {
        let mut v: Vec<PhoneInfo> = self.phones.values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

pub struct Net {
    state: Arc<Mutex<NetState>>,
    discovery: Option<UdpSocket>,
    pub pc_name: String,
    pub errors: Vec<String>,
}

fn lock(m: &Mutex<NetState>) -> MutexGuard<'_, NetState> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

pub fn pc_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "PC".into())
}

/// IPv4 address of the network card that has the default route (the Wi-Fi/LAN one).
/// No packet is sent: connecting a UDP socket only picks a route.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80")
        .or_else(|_| s.connect("192.168.1.1:80"))
        .ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(v) if !v.is_unspecified() && !v.is_loopback() => Some(v),
        _ => None,
    }
}

impl Net {
    pub fn start(audio: Arc<AudioShared>, monitor: Arc<AudioShared>) -> Net {
        let state = Arc::new(Mutex::new(NetState::default()));
        let pc_name = pc_name();
        let mut errors = Vec::new();

        match UdpSocket::bind(("0.0.0.0", SESSION_PORT)) {
            Ok(sock) => {
                let st = state.clone();
                let name = pc_name.clone();
                thread::Builder::new()
                    .name("session".into())
                    .spawn(move || session_loop(sock, st, [audio, monitor], name))
                    .expect("spawn session thread");
            }
            Err(e) => errors.push(format!(
                "Can't open UDP port {SESSION_PORT} ({e}). Is ChudMic already running?"
            )),
        }

        let discovery = match UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)) {
            Ok(sock) => {
                let _ = sock.set_broadcast(true);
                let recv = sock.try_clone().expect("clone discovery socket");
                let st = state.clone();
                let name = pc_name.clone();
                thread::Builder::new()
                    .name("discovery".into())
                    .spawn(move || discovery_loop(recv, st, name))
                    .expect("spawn discovery thread");
                Some(sock)
            }
            Err(e) => {
                errors.push(format!(
                    "Auto-discovery is off: UDP port {DISCOVERY_PORT} is busy ({e}). Use the QR code."
                ));
                None
            }
        };

        Net {
            state,
            discovery,
            pc_name,
            errors,
        }
    }

    pub fn state(&self) -> MutexGuard<'_, NetState> {
        lock(&self.state)
    }

    /// Ask a phone (with ChudMic open) to connect to this PC.
    pub fn request_connect(&self, ip: IpAddr) {
        if let Some(sock) = &self.discovery {
            let msg = discovery_msg("CONNECT", "pc", SESSION_PORT, &self.pc_name);
            let _ = sock.send_to(msg.as_bytes(), (ip, DISCOVERY_PORT));
        }
    }

    pub fn disconnect(&self) {
        self.state().kick = true;
    }
}

fn session_loop(
    sock: UdpSocket,
    state: Arc<Mutex<NetState>>,
    sinks: [Arc<AudioShared>; 2], // [virtual mic, "hear myself" monitor]
    pc_name: String,
) {
    let audio = &sinks[0];
    let clear_all = || sinks.iter().for_each(|a| a.clear());
    let push_all = |x: &[f32]| sinks.iter().for_each(|a| a.push_samples(x));
    let _ = sock.set_read_timeout(Some(Duration::from_millis(50)));
    let mut buf = [0u8; 4096];
    let mut last_ping = Instant::now();

    let mut welcome = vec![VERSION];
    welcome.extend_from_slice(pc_name.as_bytes());
    let welcome = packet(T_WELCOME, &welcome);

    let mut shifter = PitchShifter::new();
    let mut shifting = false;
    let mut pcm: Vec<f32> = Vec::with_capacity(2048);
    let mut shifted: Vec<f32> = Vec::with_capacity(4096);

    loop {
        // Housekeeping: kicks, timeouts, pings.
        {
            let mut st = lock(&state);
            let now = Instant::now();
            if st.kick {
                st.kick = false;
                if let Some(s) = st.session.take() {
                    let _ = sock.send_to(&packet(T_BYE, &[]), s.addr);
                    clear_all();
                }
            }
            if st
                .session
                .as_ref()
                .is_some_and(|s| now.duration_since(s.last_rx) > SESSION_TIMEOUT)
            {
                st.session = None;
                clear_all();
            }
            if now.duration_since(last_ping) >= PING_EVERY {
                last_ping = now;
                if let Some(s) = &st.session {
                    let flags = st.pc_muted as u8;
                    let _ = sock.send_to(&packet(T_PING, &[flags]), s.addr);
                }
            }
        }

        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(v) => v,
            // Timeouts are normal. On Windows a vanished peer also shows up here
            // as "connection reset"; just keep going.
            Err(e) if is_timeout(&e) => continue,
            Err(_) => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        if n < 3 || buf[0..2] != MAGIC {
            continue;
        }
        let now = Instant::now();
        let mut st = lock(&state);
        let ours = st.session.as_ref().is_some_and(|s| s.addr == from);

        match buf[2] {
            T_HELLO => {
                if st.session.is_none() || ours {
                    let name = if n > 4 {
                        String::from_utf8_lossy(&buf[4..n]).into_owned()
                    } else {
                        from.ip().to_string()
                    };
                    st.session = Some(Session {
                        addr: from,
                        name,
                        phone_muted: false,
                        last_rx: now,
                        last_seq: None,
                    });
                    clear_all();
                    shifter.reset();
                    let _ = sock.send_to(&welcome, from);
                } else {
                    let _ = sock.send_to(&packet(T_BUSY, &[]), from);
                }
            }
            T_AUDIO if ours && n >= 8 => {
                let s = st.session.as_mut().unwrap();
                let seq = u32::from_le_bytes([buf[3], buf[4], buf[5], buf[6]]);
                s.last_rx = now;
                s.phone_muted = buf[7] & 1 != 0;
                let stale = s
                    .last_seq
                    .is_some_and(|last| (seq.wrapping_sub(last) as i32) <= 0);
                if !stale {
                    s.last_seq = Some(seq);
                    drop(st);
                    pcm.clear();
                    pcm.extend(
                        buf[8..n]
                            .chunks_exact(2)
                            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0),
                    );
                    // Pitch shifting runs here, outside the audio callback.
                    let semitones = audio.lock().pitch;
                    if semitones.abs() < 0.01 {
                        if shifting {
                            shifting = false;
                            shifter.reset();
                        }
                        push_all(&pcm);
                    } else {
                        shifting = true;
                        shifted.clear();
                        shifter.process(&pcm, semitones, &mut shifted);
                        push_all(&shifted);
                    }
                }
            }
            T_PING if ours => {
                let s = st.session.as_mut().unwrap();
                s.last_rx = now;
                if n >= 4 {
                    s.phone_muted = buf[3] & 1 != 0;
                }
            }
            T_BYE if ours => {
                st.session = None;
                clear_all();
            }
            _ => {}
        }
    }
}

fn discovery_loop(sock: UdpSocket, state: Arc<Mutex<NetState>>, pc_name: String) {
    let _ = sock.set_read_timeout(Some(Duration::from_millis(250)));
    let ann = discovery_msg("ANN", "pc", SESSION_PORT, &pc_name);
    let rep = discovery_msg("REP", "pc", SESSION_PORT, &pc_name);
    let mut last_ann: Option<Instant> = None;
    let mut buf = [0u8; 1500];

    loop {
        let now = Instant::now();
        if last_ann.map_or(true, |t| now.duration_since(t) >= ANNOUNCE_EVERY) {
            last_ann = Some(now);
            let _ = sock.send_to(ann.as_bytes(), (Ipv4Addr::BROADCAST, DISCOVERY_PORT));
            // Windows may send 255.255.255.255 out of the wrong adapter (VPN, VirtualBox...),
            // so also try the subnet broadcast of the main adapter (assumes a /24 home network).
            if let Some(ip) = local_ipv4() {
                let o = ip.octets();
                let directed = Ipv4Addr::new(o[0], o[1], o[2], 255);
                let _ = sock.send_to(ann.as_bytes(), (directed, DISCOVERY_PORT));
            }
            lock(&state)
                .phones
                .retain(|_, p| now.duration_since(p.last_seen) < PHONE_EXPIRES);
        }

        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e) if is_timeout(&e) => continue,
            Err(_) => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let text = String::from_utf8_lossy(&buf[..n]);
        let Some(msg) = parse_discovery(&text) else {
            continue;
        };
        if msg.role != "phone" || !(msg.kind == "ANN" || msg.kind == "REP") {
            continue;
        }
        let name = if msg.name.is_empty() {
            from.ip().to_string()
        } else {
            msg.name.to_string()
        };
        lock(&state).phones.insert(
            from.ip(),
            PhoneInfo {
                ip: from.ip(),
                name,
                last_seen: Instant::now(),
            },
        );
        if msg.kind == "ANN" {
            // Unicast reply: always reaches the phone even when our broadcast took the wrong adapter.
            let _ = sock.send_to(rep.as_bytes(), from);
        }
    }
}
