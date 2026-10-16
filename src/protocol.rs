//! Wire protocol for the Cosmic Byte Blitz controller's vendor config channel.
//!
//! Everything here is pure: it builds the exact reports to send and parses the
//! reports that come back. No I/O happens in this module, which is what lets the
//! differential tests compare it byte-for-byte against the reference web app.
//!
//! Report layout (as seen on the wire, i.e. hidraw buffers *including* the report id):
//!   OUT 0x0F  [0F, 00, 00, 3C, opcode, args..., zero padding]   64 bytes total
//!   IN  0x10  [10, 00, 00, 3C, opcode, b5, addrHi, addrLo, len, data...]
//!   IN  0x01  live gamepad state
//!   OUT 0x05  rumble, exactly 5 payload bytes
//! A reply to a request carries `opcode + 1`; session commands are answered with 0x06.

pub const VENDOR_ID: u16 = 0x0281;
pub const PID_WIRED: u16 = 0x0080;
pub const PID_DONGLE: u16 = 0x0112;
pub const CONFIG_USAGE_PAGE: u16 = 0xFFF0;

pub const PROFILE_LEN: usize = 1117;
pub const CHUNK: usize = 55;
pub const CHUNK_COUNT: usize = PROFILE_LEN.div_ceil(CHUNK);
pub const PROFILE_COUNT: u8 = 4;

pub const REPORT_GAMEPAD: u8 = 0x01;
pub const REPORT_RUMBLE: u8 = 0x05;
pub const REPORT_CONFIG_OUT: u8 = 0x0F;
pub const REPORT_CONFIG_IN: u8 = 0x10;

/// Payload length of a config OUT report (report id excluded).
pub const CONFIG_PAYLOAD_LEN: usize = 63;
const HEADER: [u8; 3] = [0x00, 0x00, 0x3C];
const HEARTBEAT: [u8; 4] = [0x54, 0x34, 0x4B, 0x00];

/// Profile bytes from this offset on are lighting data; touching them needs an LED refresh.
pub const LIGHTING_START: usize = 427;

pub mod op {
    pub const ENTER: u8 = 0x01;
    pub const EXIT: u8 = 0x02;
    pub const WRITE: u8 = 0x03;
    pub const READ: u8 = 0x04;
    pub const READ_REPLY: u8 = 0x05;
    pub const ACK: u8 = 0x06;
    pub const FIRMWARE: u8 = 0x07;
    pub const FIRMWARE_REPLY: u8 = 0x08;
    pub const APPLY: u8 = 0x0C;
    pub const LIGHT: u8 = 0xE0;
    pub const CALIBRATE: u8 = 0xFD;
}

/// One OUT report. `data` excludes the report id (same convention as WebHID `sendReport`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub id: u8,
    pub data: Vec<u8>,
}

impl Report {
    /// Bytes for `hid_write`: report id followed by the payload.
    pub fn to_wire(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.data.len() + 1);
        v.push(self.id);
        v.extend_from_slice(&self.data);
        v
    }

    /// True for reports that change controller state (anything but reads and session upkeep).
    pub fn is_mutating(&self) -> bool {
        match self.id {
            REPORT_RUMBLE => true,
            REPORT_CONFIG_OUT => {
                self.data[..3] == HEADER
                    && matches!(self.data[3], op::WRITE | op::APPLY | op::LIGHT | op::CALIBRATE)
            }
            _ => false,
        }
    }
}

fn config_report(payload: &[u8]) -> Report {
    assert!(payload.len() <= CONFIG_PAYLOAD_LEN, "config payload too long: {}", payload.len());
    let mut data = vec![0u8; CONFIG_PAYLOAD_LEN];
    data[..payload.len()].copy_from_slice(payload);
    Report { id: REPORT_CONFIG_OUT, data }
}

pub fn command(opcode: u8, args: &[u8]) -> Report {
    let mut p = Vec::with_capacity(4 + args.len());
    p.extend_from_slice(&HEADER);
    p.push(opcode);
    p.extend_from_slice(args);
    config_report(&p)
}

fn wire_profile(profile: u8) -> u8 {
    assert!(profile < PROFILE_COUNT, "profile index out of range: {profile}");
    profile + 1
}

pub fn enter() -> Report {
    command(op::ENTER, &[])
}

pub fn exit(save: bool) -> Report {
    command(op::EXIT, &[save as u8])
}

pub fn heartbeat() -> Report {
    config_report(&HEARTBEAT)
}

pub fn firmware_request() -> Report {
    command(op::FIRMWARE, &[])
}

/// Chunk (address, length) pairs covering a whole profile.
pub fn profile_chunks() -> impl Iterator<Item = (usize, usize)> {
    (0..PROFILE_LEN).step_by(CHUNK).map(|a| (a, CHUNK.min(PROFILE_LEN - a)))
}

/// The read requests for one profile (0-based), in send order.
pub fn read_requests(profile: u8) -> Vec<Report> {
    let p = wire_profile(profile);
    profile_chunks()
        .map(|(a, len)| command(op::READ, &[p, (a >> 8) as u8, a as u8, len as u8]))
        .collect()
}

/// What it takes to bring the device from `base` to `next`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WritePlan {
    /// First and last differing byte (inclusive).
    pub start: usize,
    pub end: usize,
    /// WRITE reports (store) followed by APPLY reports (reload) over the same span,
    /// then an LED refresh if the span reaches into lighting data.
    pub reports: Vec<Report>,
}

/// Plan the writes for one profile. Returns `None` when nothing differs.
/// The span is the single contiguous range from the first to the last changed byte,
/// cut into 55-byte chunks starting at the first changed byte.
pub fn write_plan(profile: u8, next: &[u8; PROFILE_LEN], base: &[u8; PROFILE_LEN]) -> Option<WritePlan> {
    let p = wire_profile(profile);
    let start = (0..PROFILE_LEN).find(|&i| next[i] != base[i])?;
    let end = (0..PROFILE_LEN).rev().find(|&i| next[i] != base[i])?;

    let spans: Vec<(usize, usize)> =
        (start..=end).step_by(CHUNK).map(|a| (a, CHUNK.min(end + 1 - a))).collect();

    let mut reports = Vec::with_capacity(spans.len() * 2 + 1);
    for &(a, len) in &spans {
        let mut args = vec![p, (a >> 8) as u8, a as u8, len as u8];
        args.extend_from_slice(&next[a..a + len]);
        reports.push(command(op::WRITE, &args));
    }
    for &(a, len) in &spans {
        reports.push(command(op::APPLY, &[p, (a >> 8) as u8, a as u8, len as u8]));
    }
    if end >= LIGHTING_START {
        reports.push(set_light(true, 0));
    }
    Some(WritePlan { start, end, reports })
}

/// Make profile `profile` (0-based) the active one on the controller.
pub fn switch_profile(profile: u8) -> Report {
    let p = wire_profile(profile);
    command(op::WRITE, &[0x01, 0x04, 0x5D, 0x01, p])
}

pub fn set_light(on: bool, mode: u8) -> Report {
    command(op::LIGHT, &[on as u8, mode])
}

pub fn calibrate_gyro() -> Report {
    command(op::CALIBRATE, &[1])
}

/// Realtime rumble, raw PWM 0..=255 per motor. Deliberately not padded: this channel
/// rejects oversize reports on some firmwares.
pub fn rumble(left: u8, right: u8) -> Report {
    Report { id: REPORT_RUMBLE, data: vec![0x01, 0x00, 0x00, right, left] }
}

/// A config reply (`wire` includes the report id). `None` for anything else.
pub fn reply_opcode(wire: &[u8]) -> Option<u8> {
    (wire.len() > 4 && wire[0] == REPORT_CONFIG_IN && wire[1..4] == HEADER).then(|| wire[4])
}

/// Collects the chunked replies of one profile read.
pub struct ProfileAssembler {
    buf: [u8; PROFILE_LEN],
    seen: [bool; CHUNK_COUNT],
}

#[derive(Debug, PartialEq, Eq)]
pub enum ChunkResult {
    Accepted,
    /// A read reply that doesn't match any requested chunk; ignored.
    Rejected(String),
    NotARead,
}

impl Default for ProfileAssembler {
    fn default() -> Self {
        Self { buf: [0; PROFILE_LEN], seen: [false; CHUNK_COUNT] }
    }
}

impl ProfileAssembler {
    pub fn accept(&mut self, wire: &[u8]) -> ChunkResult {
        if reply_opcode(wire) != Some(op::READ_REPLY) {
            return ChunkResult::NotARead;
        }
        if wire.len() < 9 {
            return ChunkResult::Rejected(format!("read reply too short ({} bytes)", wire.len()));
        }
        let addr = ((wire[6] as usize) << 8) | wire[7] as usize;
        if addr >= PROFILE_LEN || !addr.is_multiple_of(CHUNK) {
            return ChunkResult::Rejected(format!("read reply for unexpected address {addr}"));
        }
        let want = CHUNK.min(PROFILE_LEN - addr);
        if wire.len() - 9 < want {
            return ChunkResult::Rejected(format!("read reply at {addr} carries {} of {want} bytes", wire.len() - 9));
        }
        self.buf[addr..addr + want].copy_from_slice(&wire[9..9 + want]);
        self.seen[addr / CHUNK] = true;
        ChunkResult::Accepted
    }

    pub fn received(&self) -> usize {
        self.seen.iter().filter(|&&s| s).count()
    }

    pub fn finish(&self) -> Option<[u8; PROFILE_LEN]> {
        self.seen.iter().all(|&s| s).then_some(self.buf)
    }
}

/// Firmware version digits from a FIRMWARE_REPLY, joined with dots (e.g. "0.2.1.2").
pub fn decode_firmware(wire: &[u8]) -> Option<String> {
    if reply_opcode(wire) != Some(op::FIRMWARE_REPLY) {
        return None;
    }
    let window = wire.get(5..13.min(wire.len()))?;
    let digits: Vec<String> =
        window.iter().filter(|b| b.is_ascii_digit()).map(|&b| (b as char).to_string()).collect();
    (!digits.is_empty()).then(|| digits.join("."))
}

/// The firmware pads its version with a leading "0" part: "0.2.1.2" is 2.1.2.
pub fn normalize_firmware(v: &str) -> String {
    let parts: Vec<&str> = v.trim().split('.').collect();
    if parts.len() >= 4 && parts[0] == "0" { parts[1..].join(".") } else { v.to_string() }
}

/// Newest Blitz firmware known when this tool was written (from the Orbit manifest, v2.93.0).
pub const LATEST_KNOWN_FIRMWARE: &str = "2.1.2";
pub const FIRMWARE_DOWNLOAD_URL: &str = "https://www.thecosmicbyte.com/downloaddrivers/";

/// First `N(.N(.N))` run in the string; missing parts are 0.
fn parse_version(v: &str) -> Option<[u32; 3]> {
    let v = normalize_firmware(v);
    let b = v.as_bytes();
    let mut i = b.iter().position(u8::is_ascii_digit)?;
    let mut out = [0u32; 3];
    for (k, slot) in out.iter_mut().enumerate() {
        if k > 0 {
            if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
                i += 1;
            } else {
                break;
            }
        }
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        *slot = v[start..i].parse().ok()?;
    }
    Some(out)
}

pub fn firmware_is_newer(latest: &str, installed: &str) -> bool {
    matches!((parse_version(latest), parse_version(installed)), (Some(a), Some(b)) if a > b)
}

/// Does the HID report descriptor declare a vendor collection (usage page 0xFFF0)
/// containing OUT report 0x0F? Without it the controller is in a mode (XInput, Bluetooth)
/// that doesn't expose configuration, and nothing should be sent.
pub fn descriptor_has_config_channel(desc: &[u8]) -> bool {
    let mut usage_page: u32 = 0;
    let mut report_id: u32 = 0;
    let mut depth = 0u32;
    let mut top_page: u32 = 0;
    let mut i = 0;
    while i < desc.len() {
        let prefix = desc[i];
        if prefix == 0xFE {
            // long item: [FE, size, tag, data...]
            let size = *desc.get(i + 1).unwrap_or(&0) as usize;
            i += 3 + size;
            continue;
        }
        let size = match prefix & 0x03 { 3 => 4, n => n as usize };
        let Some(raw) = desc.get(i + 1..i + 1 + size) else { return false };
        let value = raw.iter().rev().fold(0u32, |acc, &b| (acc << 8) | b as u32);
        match prefix & 0xFC {
            0x04 => usage_page = value,  // Usage Page (global)
            0x84 => report_id = value,   // Report ID (global)
            0xA0 => {
                // Collection
                if depth == 0 {
                    top_page = usage_page;
                }
                depth += 1;
            }
            0xC0 => depth = depth.saturating_sub(1), // End Collection
            // Output
            0x90 if top_page == CONFIG_USAGE_PAGE as u32 && report_id == REPORT_CONFIG_OUT as u32 => return true,
            _ => {}
        }
        i += 1 + size;
    }
    false
}

/// Decoded 0x01 gamepad report. Bit positions follow the reference configurator's live preview.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gamepad {
    /// -1.0..=1.0, +y is down.
    pub left: (f32, f32),
    pub right: (f32, f32),
    pub lt: u8,
    pub rt: u8,
    pub a: bool,
    pub b: bool,
    pub x: bool,
    pub y: bool,
    pub lb: bool,
    pub rb: bool,
    pub select: bool,
    pub start: bool,
    pub home: bool,
    pub capture: bool,
    pub up: bool,
    pub down: bool,
    pub left_dpad: bool,
    pub right_dpad: bool,
}

pub fn decode_gamepad(wire: &[u8]) -> Option<Gamepad> {
    if wire.first() != Some(&REPORT_GAMEPAD) || wire.len() < 10 {
        return None;
    }
    let d = &wire[1..];
    let ax = |i: usize| ((d[i] as f32 - 128.0) / 127.0).clamp(-1.0, 1.0);
    let (b4, b5, b6) = (d[4], d[5], d[6]);
    let hat = b4 & 0x0F; // 0=U 1=UR 2=R 3=DR 4=D 5=DL 6=L 7=UL, 0xF neutral
    Some(Gamepad {
        left: (ax(0), ax(1)),
        right: (ax(2), ax(3)),
        lt: d[7],
        rt: d[8],
        x: b4 & 0x10 != 0,
        a: b4 & 0x20 != 0,
        b: b4 & 0x40 != 0,
        y: b4 & 0x80 != 0,
        lb: b5 & 0x01 != 0,
        rb: b5 & 0x02 != 0,
        select: b5 & 0x10 != 0,
        start: b5 & 0x20 != 0,
        home: b6 & 0x01 != 0,
        capture: b6 & 0x02 != 0,
        up: matches!(hat, 0 | 1 | 7),
        right_dpad: matches!(hat, 1..=3),
        down: matches!(hat, 3..=5),
        left_dpad: matches!(hat, 5..=7),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Report descriptor read from a real wired Blitz (0281:0080).
    const BLITZ_WIRED_DESCRIPTOR: &str = "05010906a1018503050719e029e715002501750195088102750895018101050719002aff00150026ff00750895068100050819012903250175019503910295059101c0060c000901a101850219002bffff0000150027ffff0000751095028100c005010902a10185090901a1000509190129051500250195057501810295017503810105010930093116018026ff7f75109502810609381581257f75089501810695018103c0c006f0ff0940a1010600ff09008510150026ff00350046ff007508953f81020600ff09008501150026ff00350046ff007508953f810285050922951f9102850f0922953f9102c0";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn real_descriptor_exposes_config_channel() {
        assert!(descriptor_has_config_channel(&hex(BLITZ_WIRED_DESCRIPTOR)));
    }

    #[test]
    fn descriptor_without_vendor_collection_is_rejected() {
        // Same descriptor cut before the 0xFFF0 collection.
        let full = hex(BLITZ_WIRED_DESCRIPTOR);
        let cut = full.windows(3).position(|w| w == [0x06, 0xF0, 0xFF]).unwrap();
        assert!(!descriptor_has_config_channel(&full[..cut]));
    }

    #[test]
    fn chunking_covers_profile_exactly() {
        let chunks: Vec<_> = profile_chunks().collect();
        assert_eq!(chunks.len(), CHUNK_COUNT);
        assert_eq!(chunks.iter().map(|c| c.1).sum::<usize>(), PROFILE_LEN);
        assert_eq!(*chunks.last().unwrap(), (1100, 17));
    }

    #[test]
    fn write_report_fills_payload_exactly() {
        let base = [0u8; PROFILE_LEN];
        let mut next = base;
        next[..CHUNK].fill(0xAA);
        let plan = write_plan(0, &next, &base).unwrap();
        // 3 header + opcode + 4 args + 55 data = 63, the full payload.
        assert_eq!(plan.reports[0].data.len(), CONFIG_PAYLOAD_LEN);
        assert_eq!(plan.reports[0].data[62], 0xAA);
    }

    #[test]
    fn versions() {
        assert_eq!(normalize_firmware("0.2.1.2"), "2.1.2");
        assert_eq!(normalize_firmware("2.1.2"), "2.1.2");
        assert!(firmware_is_newer("2.1.2", "0.2.1.1"));
        assert!(!firmware_is_newer("2.1.2", "2.1.2"));
        assert!(!firmware_is_newer("2.1.2", "0.2.2.0"));
    }

    #[test]
    #[should_panic]
    fn profile_index_is_bounds_checked() {
        switch_profile(4);
    }
}
