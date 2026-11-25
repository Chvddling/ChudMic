//! Wire protocol shared with the Android app. Keep in sync with Proto.kt.
//!
//! Discovery (UDP port 47800, plain text):
//!   CHUDMIC|1|ANN|<role>|<port>|<name>      broadcast every ~1.5 s
//!   CHUDMIC|1|REP|<role>|<port>|<name>      unicast answer to an ANN
//!   CHUDMIC|1|CONNECT|pc|<port>|<name>      PC asks a phone to connect to it
//!   role is "pc" or "phone".
//!
//! Session (UDP port 47801 on the PC, binary, every packet starts with "CM" + type byte):
//!   H  phone->pc  hello:   [ver u8][phone name utf8]
//!   W  pc->phone  welcome: [ver u8][pc name utf8]
//!   B  pc->phone  busy (another phone is connected)
//!   A  phone->pc  audio:   [seq u32 LE][flags u8][PCM s16 LE mono 48 kHz]
//!   P  both       ping:    [flags u8]   (bit0 = this side is muted), every 500 ms
//!   X  both       bye

pub const DISCOVERY_PORT: u16 = 47800;
pub const SESSION_PORT: u16 = 47801;
pub const VERSION: u8 = 1;
pub const SAMPLE_RATE: u32 = 48_000;

pub const MAGIC: [u8; 2] = *b"CM";
pub const T_HELLO: u8 = b'H';
pub const T_WELCOME: u8 = b'W';
pub const T_BUSY: u8 = b'B';
pub const T_AUDIO: u8 = b'A';
pub const T_PING: u8 = b'P';
pub const T_BYE: u8 = b'X';

pub fn packet(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(3 + payload.len());
    v.extend_from_slice(&MAGIC);
    v.push(kind);
    v.extend_from_slice(payload);
    v
}

/// Discovery text message. Name goes last so it may contain anything except a newline.
pub fn discovery_msg(kind: &str, role: &str, port: u16, name: &str) -> String {
    format!("CHUDMIC|1|{kind}|{role}|{port}|{name}")
}

pub struct DiscoveryMsg<'a> {
    pub kind: &'a str,
    pub role: &'a str,
    pub port: u16,
    pub name: &'a str,
}

pub fn parse_discovery(text: &str) -> Option<DiscoveryMsg<'_>> {
    let mut it = text.splitn(6, '|');
    if it.next()? != "CHUDMIC" {
        return None;
    }
    let _version = it.next()?;
    let kind = it.next()?;
    let role = it.next()?;
    let port = it.next()?.parse().unwrap_or(SESSION_PORT);
    let name = it.next().unwrap_or("");
    Some(DiscoveryMsg {
        kind,
        role,
        port,
        name,
    })
}
