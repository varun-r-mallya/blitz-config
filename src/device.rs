//! HID transport. One worker thread owns the device handle, so every report goes out
//! in a strict order and reads/writes can never overlap.
//!
//! Safety rails, on top of the byte-exact protocol:
//! - a device is only opened if its report descriptor has the vendor config channel;
//! - a profile is only handed to the UI if every chunk arrived intact;
//! - each profile read is saved to the backup directory before it can be edited;
//! - in dry-run mode, state-changing reports are logged and never sent;
//! - every report sent and every config reply received is appended to a log file.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hidapi::{HidApi, HidDevice};

use crate::protocol::{self as p, PROFILE_LEN, Report, op};

pub type ProfileBytes = Box<[u8; PROFILE_LEN]>;

#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: CString,
    pub product_id: u16,
    pub transport: &'static str,
}

#[derive(Debug)]
pub enum Discovery {
    Found(Candidate),
    /// A Blitz is attached but its config channel isn't exposed (XInput / Bluetooth).
    NoConfigChannel,
    /// The config interface exists but couldn't be opened (usually permissions).
    CannotOpen(String),
    NotFound,
}

fn transport_name(pid: u16) -> &'static str {
    match pid {
        p::PID_WIRED => "Wired USB",
        p::PID_DONGLE => "2.4 GHz",
        _ => "Unknown",
    }
}

pub fn discover() -> Result<Discovery, String> {
    let api = HidApi::new().map_err(|e| format!("HID init failed: {e}"))?;
    let mut saw_blitz = false;
    let mut open_error = None;
    for info in api.device_list() {
        if info.vendor_id() != p::VENDOR_ID || !matches!(info.product_id(), p::PID_WIRED | p::PID_DONGLE) {
            continue;
        }
        saw_blitz = true;
        if info.usage_page() != p::CONFIG_USAGE_PAGE {
            continue;
        }
        let dev = match api.open_path(info.path()) {
            Ok(d) => d,
            Err(e) => {
                open_error = Some(format!("{}: {e}", info.path().to_string_lossy()));
                continue;
            }
        };
        let mut desc = [0u8; 4096];
        let n = dev.get_report_descriptor(&mut desc).unwrap_or(0);
        if p::descriptor_has_config_channel(&desc[..n]) {
            return Ok(Discovery::Found(Candidate {
                path: info.path().to_owned(),
                product_id: info.product_id(),
                transport: transport_name(info.product_id()),
            }));
        }
    }
    Ok(match (saw_blitz, open_error) {
        (_, Some(e)) => Discovery::CannotOpen(e),
        (true, None) => Discovery::NoConfigChannel,
        (false, None) => Discovery::NotFound,
    })
}

pub enum Cmd {
    ReadProfile(u8),
    Write { profile: u8, next: ProfileBytes, base: ProfileBytes },
    SwitchProfile(u8),
    /// Raw PWM per motor; `ms` > 0 schedules an automatic stop.
    Rumble { left: u8, right: u8, ms: u64 },
    CalibrateGyro,
    Disconnect,
}

pub enum Event {
    Connected { firmware: Option<String> },
    ProfileLoaded { profile: u8, data: ProfileBytes, backup: Option<PathBuf> },
    /// Outcome of a write to `profile`.
    Written { profile: u8, result: WriteResult },
    Error(String),
    Disconnected(Option<String>),
}

/// Every write is read back and compared with what was intended.
pub enum WriteResult {
    /// The controller holds exactly the intended bytes (in dry-run: simulated).
    Verified(ProfileBytes),
    /// The read-back differs; `actual` is what the controller really holds.
    Mismatch { actual: ProfileBytes, detail: String },
    /// The read-back itself failed, so the controller's state is unknown.
    Unverified(String),
}

#[derive(Default)]
pub struct Live {
    pub gamepad: Option<p::Gamepad>,
}

pub struct Handle {
    pub cmd: Sender<Cmd>,
    pub events: Receiver<Event>,
    pub live: Arc<Mutex<Live>>,
    /// While set, trigger pressure drives the grip motors (vibration test).
    pub grip_test: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn send(&self, c: Cmd) {
        let _ = self.cmd.send(c);
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.cmd.send(Cmd::Disconnect);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("blitz-config")
}

fn unix_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

pub fn connect(candidate: Candidate, dry_run: bool, repaint: impl Fn() + Send + 'static) -> Result<Handle, String> {
    let api = HidApi::new().map_err(|e| format!("HID init failed: {e}"))?;
    let dev = api.open_path(&candidate.path).map_err(|e| {
        format!("Could not open {}: {e}. Close any browser tab using the controller and check device permissions.", candidate.path.to_string_lossy())
    })?;

    let dir = data_dir();
    let _ = fs::create_dir_all(dir.join("backups"));
    // Two sessions interleaving commands on one controller is never safe; refuse a second one.
    let lock = File::create(dir.join("session.lock")).map_err(|e| format!("Could not create session lock: {e}"))?;
    if lock.try_lock().is_err() {
        return Err("Another blitz-config window is already connected to the controller. Close it first.".into());
    }
    let log = OpenOptions::new().create(true).append(true).open(dir.join("hid.log")).ok();

    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (ev_tx, ev_rx) = mpsc::channel();
    let live = Arc::new(Mutex::new(Live::default()));
    let grip_test = Arc::new(AtomicBool::new(false));

    let mut w = Worker {
        _lock: lock,
        dev,
        dry_run,
        log,
        backups: dir.join("backups"),
        events: ev_tx,
        live: live.clone(),
        grip_test: grip_test.clone(),
        repaint: Box::new(repaint),
        next_heartbeat: None,
        rumble_off_at: None,
        waiting: None,
        assembler: None,
        grip: GripState::default(),
    };
    let thread = std::thread::Builder::new()
        .name("blitz-hid".into())
        .spawn(move || w.run(cmd_rx))
        .map_err(|e| e.to_string())?;

    Ok(Handle { cmd: cmd_tx, events: ev_rx, live, grip_test, thread: Some(thread) })
}

#[derive(Default)]
struct GripState {
    left: u8,
    right: u8,
    last: Option<Instant>,
}

struct Worker {
    /// Held for the whole session; released when the worker exits.
    _lock: File,
    dev: HidDevice,
    dry_run: bool,
    log: Option<File>,
    backups: PathBuf,
    events: Sender<Event>,
    live: Arc<Mutex<Live>>,
    grip_test: Arc<AtomicBool>,
    repaint: Box<dyn Fn() + Send>,
    next_heartbeat: Option<Instant>,
    rumble_off_at: Option<Instant>,
    /// Reply opcode being waited for, and the reply once it arrives.
    waiting: Option<(u8, Option<Vec<u8>>)>,
    assembler: Option<p::ProfileAssembler>,
    grip: GripState,
}

type Res<T> = Result<T, String>;

impl Worker {
    fn log_line(&mut self, dir: &str, bytes: &[u8]) {
        if let Some(f) = &mut self.log {
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
            let _ = writeln!(f, "{} {dir} {hex}", unix_ms());
        }
    }

    fn send(&mut self, r: &Report) -> Res<()> {
        let wire = r.to_wire();
        if self.dry_run && r.is_mutating() {
            self.log_line("DRY", &wire);
            return Ok(());
        }
        if *r != p::heartbeat() {
            self.log_line("OUT", &wire);
        }
        self.dev.write(&wire).map(|_| ()).map_err(|e| format!("HID write failed: {e}"))
    }

    /// Service the device until `until`: heartbeats, rumble auto-stop, and incoming reports.
    fn pump_until(&mut self, until: Instant) -> Res<()> {
        let mut buf = [0u8; 65];
        loop {
            let now = Instant::now();
            if let Some(t) = self.next_heartbeat
                && now >= t
            {
                self.send(&p::heartbeat())?;
                self.next_heartbeat = Some(t + Duration::from_secs(1));
            }
            if let Some(t) = self.rumble_off_at
                && now >= t
            {
                self.rumble_off_at = None;
                let _ = self.send(&p::rumble(0, 0));
            }
            let remaining = until.saturating_duration_since(Instant::now());
            let ms = remaining.as_millis().min(5) as i32;
            let n = self.dev.read_timeout(&mut buf, ms).map_err(|e| format!("HID read failed: {e}"))?;
            if n > 0 {
                self.dispatch(&buf[..n])?;
            }
            if Instant::now() >= until {
                return Ok(());
            }
        }
    }

    fn pump(&mut self, ms: u64) -> Res<()> {
        self.pump_until(Instant::now() + Duration::from_millis(ms))
    }

    fn dispatch(&mut self, wire: &[u8]) -> Res<()> {
        if let Some(g) = p::decode_gamepad(wire) {
            self.live.lock().unwrap().gamepad = Some(g);
            self.grip_step(g.lt, g.rt)?;
            (self.repaint)();
            return Ok(());
        }
        let Some(opcode) = p::reply_opcode(wire) else {
            self.log_line("IN?", wire); // anything unexpected is recorded, never acted on
            return Ok(());
        };
        self.log_line("IN ", wire);
        if let Some(a) = &mut self.assembler
            && let p::ChunkResult::Rejected(why) = a.accept(wire)
        {
            self.log_line("REJ", why.as_bytes());
        }
        if let Some((want, slot)) = &mut self.waiting
            && *want == opcode
            && slot.is_none()
        {
            *slot = Some(wire.to_vec());
        }
        Ok(())
    }

    /// Send `r` and wait up to `ms` for a reply with opcode `reply`.
    fn request(&mut self, r: &Report, reply: u8, ms: u64) -> Res<Option<Vec<u8>>> {
        self.waiting = Some((reply, None));
        self.send(r)?;
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            self.pump(5)?;
            if let Some((_, Some(_))) = &self.waiting {
                break;
            }
        }
        Ok(self.waiting.take().and_then(|(_, r)| r))
    }

    fn rumble(&mut self, left: u8, right: u8, ms: u64) -> Res<()> {
        self.rumble_off_at = None;
        self.send(&p::rumble(left, right))?;
        if ms > 0 {
            self.rumble_off_at = Some(Instant::now() + Duration::from_millis(ms));
        }
        Ok(())
    }

    /// Vibration test: trigger pressure drives the motors, refreshed on change or every 220 ms.
    fn grip_step(&mut self, lt: u8, rt: u8) -> Res<()> {
        if !self.grip_test.load(Ordering::Relaxed) {
            // Turning the test off sends its own stop (Cmd::Rumble 0,0).
            self.grip = GripState::default();
            return Ok(());
        }
        let now = Instant::now();
        let changed = (lt as i32 - self.grip.left as i32).abs() > 12 || (rt as i32 - self.grip.right as i32).abs() > 12;
        let stale = self.grip.last.is_none_or(|t| now - t > Duration::from_millis(220));
        if changed || stale {
            self.grip = GripState { left: lt, right: rt, last: Some(now) };
            self.rumble(lt, rt, 300)?;
        }
        Ok(())
    }

    fn read_profile(&mut self, profile: u8) -> Res<ProfileBytes> {
        self.assembler = Some(p::ProfileAssembler::default());
        for r in p::read_requests(profile) {
            self.send(&r)?;
            self.pump(9)?;
        }
        let deadline = Instant::now() + Duration::from_millis(3000);
        loop {
            let a = self.assembler.as_ref().unwrap();
            if let Some(data) = a.finish() {
                self.assembler = None;
                return Ok(Box::new(data));
            }
            if Instant::now() >= deadline {
                let got = a.received();
                self.assembler = None;
                return Err(format!(
                    "Profile {} read incomplete ({got}/{} chunks). Nothing was changed; try Refresh.",
                    profile + 1,
                    p::CHUNK_COUNT
                ));
            }
            self.pump(5)?;
        }
    }

    fn backup(&self, profile: u8, data: &[u8]) -> Option<PathBuf> {
        let path = self.backups.join(format!("p{}-{}.bin", profile + 1, unix_ms()));
        fs::write(&path, data).ok().map(|_| path)
    }

    fn write(&mut self, profile: u8, next: &[u8; PROFILE_LEN], base: &[u8; PROFILE_LEN]) -> Res<()> {
        let Some(plan) = p::write_plan(profile, next, base) else { return Ok(()) };
        for r in &plan.reports {
            self.send(r)?;
            let gap = match r.data[3] {
                op::WRITE => 9,
                op::APPLY => 7,
                _ => 0,
            };
            if gap > 0 {
                self.pump(gap)?;
            }
        }
        Ok(())
    }

    fn write_verified(&mut self, profile: u8, next: ProfileBytes, base: &[u8; PROFILE_LEN]) -> Res<WriteResult> {
        if p::write_plan(profile, &next, base).is_none() {
            return Ok(WriteResult::Verified(next));
        }
        self.write(profile, &next, base)?;
        if self.dry_run {
            return Ok(WriteResult::Verified(next));
        }
        let actual = match self.read_profile_retrying(profile) {
            Ok(a) => a,
            Err(e) => return Ok(WriteResult::Unverified(format!("Wrote profile {} but couldn't read it back: {e}", profile + 1))),
        };
        if actual == next {
            return Ok(WriteResult::Verified(actual));
        }
        let bad: Vec<usize> = (0..PROFILE_LEN).filter(|&i| actual[i] != next[i]).collect();
        self.log_line("MIS", format!("{bad:?}").as_bytes());
        let detail = format!(
            "Profile {} read back differently from what was written ({} byte(s), first at offset {}). \
             Auto-apply is paused and the editor now shows what the controller holds.",
            profile + 1,
            bad.len(),
            bad[0]
        );
        Ok(WriteResult::Mismatch { actual, detail })
    }

    /// Reads have no side effects, so an incomplete one is retried once.
    fn read_profile_retrying(&mut self, profile: u8) -> Res<ProfileBytes> {
        match self.read_profile(profile) {
            Ok(d) => Ok(d),
            Err(_) => {
                self.pump(200)?;
                self.read_profile(profile)
            }
        }
    }

    fn handle(&mut self, c: Cmd) -> Res<bool> {
        match c {
            Cmd::ReadProfile(i) => self.load(i)?,
            Cmd::Write { profile, next, base } => {
                let result = self.write_verified(profile, next, &base)?;
                let _ = self.events.send(Event::Written { profile, result });
            }
            Cmd::SwitchProfile(i) => {
                self.send(&p::switch_profile(i))?;
                self.pump(40)?;
            }
            Cmd::Rumble { left, right, ms } => self.rumble(left, right, ms)?,
            Cmd::CalibrateGyro => self.send(&p::calibrate_gyro())?,
            Cmd::Disconnect => return Ok(false),
        }
        (self.repaint)();
        Ok(true)
    }

    fn load(&mut self, i: u8) -> Res<()> {
        match self.read_profile_retrying(i) {
            Ok(data) => {
                let backup = self.backup(i, &data[..]);
                let _ = self.events.send(Event::ProfileLoaded { profile: i, data, backup });
            }
            Err(e) => {
                let _ = self.events.send(Event::Error(e));
            }
        }
        Ok(())
    }

    fn session(&mut self, cmds: &Receiver<Cmd>) -> Res<()> {
        // The controller can ignore commands for about a second after it is opened.
        // Entering config mode and the version query are side-effect free, so retry them.
        for _ in 0..4 {
            if self.request(&p::enter(), op::ACK, 600)?.is_some() {
                break;
            }
        }
        self.pump(20)?;
        self.next_heartbeat = Some(Instant::now() + Duration::from_secs(1));

        let mut firmware = None;
        for _ in 0..3 {
            firmware = self.request(&p::firmware_request(), op::FIRMWARE_REPLY, 800)?
                .and_then(|r| p::decode_firmware(&r))
                .map(|v| p::normalize_firmware(&v));
            if firmware.is_some() {
                break;
            }
        }
        let _ = self.events.send(Event::Connected { firmware });
        (self.repaint)();

        self.load(0)?;
        loop {
            match cmds.try_recv() {
                Ok(c) => {
                    if !self.handle(c)? {
                        return Ok(());
                    }
                }
                Err(TryRecvError::Empty) => self.pump(10)?,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }

    fn run(&mut self, cmds: Receiver<Cmd>) {
        let _ = self.dev.set_blocking_mode(false);
        let result = self.session(&cmds);
        self.next_heartbeat = None;
        if result.is_ok() {
            // Leave config mode without the extra save flag, as the reference app does.
            let _ = self.request(&p::exit(false), op::ACK, 600);
            let _ = self.pump(40);
        }
        let _ = self.events.send(Event::Disconnected(result.err()));
        (self.repaint)();
    }
}
