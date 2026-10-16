//! Typed view over the 1117-byte Blitz profile.
//!
//! Every setter changes exactly the bytes the reference configurator changes for the
//! same user action, including its side effects (e.g. an explicit "enable" byte, or a
//! stick range falling back to 10–100% when the stored one is invalid). The
//! differential tests in `tests/differential.rs` hold this module to that.

use crate::protocol::PROFILE_LEN;

pub const NAME: usize = 0;
pub const NAME_LEN: usize = 32;
/// [0] = pick-up-to-wake (0 = on), [1] = report rate
pub const POWER: usize = 32;
/// [0] = left motor level, [1] = right motor level (percent)
pub const MOTOR: usize = 34;
/// 10-byte gyro block
pub const SENSOR: usize = 41;
/// [0],[1] = swap stick/D-pad, [2],[3] = hair trigger, [4] = diagonal lock, [6] = auto-sleep minutes
pub const RESERVE: usize = 51;
pub const BUTTONS: usize = 58;
pub const BUTTON_STRIDE: usize = 4;
pub const PADDLES: usize = 122;
pub const PADDLE_STRIDE: usize = 125;
pub const TRIGGERS: usize = 372;
pub const TRIGGER_STRIDE: usize = 11;
pub const STICKS: usize = 394;
pub const STICK_STRIDE: usize = 16;
pub const LIGHT_INDEX: usize = 426;
pub const LIGHTS: usize = 427;
pub const LIGHT_STRIDE: usize = 172;
pub const LIGHT_FRAMES: usize = 8;
pub const LIGHT_LEDS: usize = 7;

pub const VIBRATION_LEVELS: [u8; 4] = [0, 30, 70, 100];
pub const SLEEP_MINUTES: [u8; 4] = [0, 1, 5, 10];

/// Gamepad button codes used by remaps and the gyro activation button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Up, Down, Left, Right, Lb, Rb, Lsb, Rsb, A, B, X, Y, Home, Select, Start, Capture, L4, R4, Lt, Rt,
}

impl Button {
    pub const fn code(self) -> u8 {
        use Button::*;
        match self {
            Up => 1, Down => 2, Left => 3, Right => 4, Lb => 5, Rb => 6, Lsb => 7, Rsb => 8,
            A => 9, B => 10, X => 11, Y => 12, Home => 13, Select => 14, Start => 15, Capture => 16,
            L4 => 17, R4 => 18, Lt => 19, Rt => 20,
        }
    }

    pub fn from_code(c: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.code() == c)
    }

    pub const ALL: [Button; 20] = {
        use Button::*;
        [Up, Down, Left, Right, Lb, Rb, Lsb, Rsb, A, B, X, Y, Home, Select, Start, Capture, L4, R4, Lt, Rt]
    };

    pub const fn label(self) -> &'static str {
        use Button::*;
        match self {
            Up => "UP", Down => "DOWN", Left => "LEFT", Right => "RIGHT", Lb => "LB", Rb => "RB",
            Lsb => "LSB", Rsb => "RSB", A => "A", B => "B", X => "X", Y => "Y", Home => "HOME",
            Select => "SELECT", Start => "START", Capture => "CAPTURE", L4 => "L4", R4 => "R4",
            Lt => "LT", Rt => "RT",
        }
    }

    /// Byte offset of this button's mapping record, if it can be remapped.
    fn record(self) -> usize {
        use Button::*;
        const BASIC: [Button; 16] = [Up, Down, Left, Right, Lb, Rb, Lsb, Rsb, A, B, X, Y, Home, Select, Start, Capture];
        match self {
            L4 => PADDLES,
            R4 => PADDLES + PADDLE_STRIDE,
            Lt => TRIGGERS,
            Rt => TRIGGERS + TRIGGER_STRIDE,
            b => BUTTONS + BASIC.iter().position(|&x| x == b).unwrap() * BUTTON_STRIDE,
        }
    }

    /// Offset of the target code within the record (triggers keep it further in).
    fn value_offset(self) -> usize {
        if matches!(self, Button::Lt | Button::Rt) { 5 } else { 3 }
    }
}

/// Remappable buttons, grouped as in the reference UI.
pub const MAP_GROUPS: [(&str, &[Button]); 4] = {
    use Button::*;
    [
        ("D-pad", &[Up, Down, Left, Right]),
        ("Face", &[A, B, X, Y]),
        ("Shoulder & Trigger", &[Lb, Rb, Lt, Rt]),
        ("Stick & System", &[Lsb, Rsb, Home, Select, Start, Capture]),
    ]
};

/// Every source the reference UI lets you remap (and resets).
pub const MAP_SOURCES: [Button; 18] = {
    use Button::*;
    [Up, Down, Left, Right, A, B, X, Y, Lb, Rb, Lt, Rt, Lsb, Rsb, Home, Select, Start, Capture]
};

/// Targets offered in the remap picker, in picker order.
pub const MAP_TARGETS: [Mapping; 20] = {
    use Button::*;
    use Mapping::*;
    [
        Default, Off, To(Up), To(Down), To(Left), To(Right), To(A), To(B), To(X), To(Y),
        To(Lb), To(Rb), To(Lt), To(Rt), To(Lsb), To(Rsb), To(Home), To(Select), To(Start), To(Capture),
    ]
};

/// Gyro activation-button choices (code 0 = none).
pub const GYRO_BUTTONS: [u8; 17] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 19, 20, 14, 15];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapping {
    Default,
    Off,
    To(Button),
}

impl Mapping {
    pub fn label(self) -> &'static str {
        match self {
            Mapping::Default => "Default",
            Mapping::Off => "Off",
            Mapping::To(b) => b.label(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// A dual-handle range, in percent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub initial: i32,
    pub max: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stick {
    pub range: Range,
    /// Raw (square) output; the UI shows the inverse as "radial trace".
    pub raw: bool,
}

/// Which handle of a dual range is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Initial,
    Max,
}

/// Move one handle of a range to `pct` (0..=100), keeping the handles at least 1% apart.
pub fn drag_range(mut r: Range, handle: Handle, pct: i32) -> Range {
    match handle {
        Handle::Initial => r.initial = 0.max(pct.min(r.max - 1)),
        Handle::Max => r.max = 100.min(pct.max(r.initial + 1)),
    }
    r
}

/// A collapsed range (max <= initial) pulled 1% apart: initial drops to max-1, or max rises
/// to 1 when initial is 0. The reference slider does this (and stores the result) whenever
/// it repaints: when shown, and again after every drag step. Storing can clamp the healed
/// value, so a range can still be collapsed after one pass; the next repaint heals again.
pub fn heal_range(r: Range) -> Option<Range> {
    (r.max - r.initial < 1).then(|| {
        if r.initial >= 1 { Range { initial: r.max - 1, ..r } } else { Range { max: 100.min(r.initial + 1), ..r } }
    })
}

/// Gyro activation behaviour. Discriminants are the values the UI encodes; note the
/// on-device encoding is not in reading order (see `set_gyro_method`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GyroMethod {
    Off = 0,
    PressToTurnOn = 1,
    AlwaysOn = 2,
    PressToTurnOff = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GyroOutput {
    Off = 0,
    LeftStick = 1,
    RightStick = 2,
}

/// Response-curve presets: SENSOR bytes 5..=8 (origin1, target1, origin2, target2).
pub const CURVE_PRESETS: [[u8; 4]; 3] = [[80, 40, 255, 160], [47, 24, 255, 127], [30, 12, 255, 100]];
pub const CURVE_CUSTOM: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurvePoint {
    P1,
    P2,
}

/// JavaScript `Math.round` (ties toward +infinity).
pub fn js_round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Profile(pub [u8; PROFILE_LEN]);

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Profile({:?})", self.name())
    }
}

impl Profile {
    fn bit(&self, byte: usize, n: u8) -> bool {
        self.0[byte] & (1 << n) != 0
    }

    fn set_bit(&mut self, byte: usize, n: u8, on: bool) {
        if on {
            self.0[byte] |= 1 << n;
        } else {
            self.0[byte] &= !(1 << n);
        }
    }

    // ---- name ----

    /// Bytes up to the first NUL, one char per byte (as the reference UI displays it).
    pub fn name(&self) -> String {
        self.0[NAME..NAME + NAME_LEN].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
    }

    /// First 24 UTF-16 units of `name`, UTF-8 encoded, truncated/zero-padded to 32 bytes.
    pub fn set_name(&mut self, name: &str) {
        let units: Vec<u16> = name.encode_utf16().take(24).collect();
        let bytes = String::from_utf16_lossy(&units).into_bytes();
        for i in 0..NAME_LEN {
            self.0[NAME + i] = bytes.get(i).copied().unwrap_or(0);
        }
    }

    // ---- button mappings ----

    pub fn mapping(&self, src: Button) -> Mapping {
        let off = src.record();
        if !self.bit(off + 1, 2) {
            return Mapping::Default;
        }
        match self.0[off + src.value_offset()] {
            255 => Mapping::Off,
            c => Button::from_code(c).map_or(Mapping::Default, Mapping::To),
        }
    }

    pub fn set_mapping(&mut self, src: Button, target: Mapping) {
        let off = src.record();
        self.0[off] = 1;
        match target {
            Mapping::Default => self.set_bit(off + 1, 2, false),
            Mapping::Off => {
                self.set_bit(off + 1, 2, true);
                self.0[off + src.value_offset()] = 255;
            }
            Mapping::To(b) => {
                self.set_bit(off + 1, 2, true);
                self.0[off + src.value_offset()] = b.code();
            }
        }
    }

    // ---- sticks ----

    fn stick_at(s: Side) -> usize {
        match s {
            Side::Left => STICKS,
            Side::Right => STICKS + STICK_STRIDE,
        }
    }

    /// Falls back to 10–100% when the stored range is unset or inverted.
    pub fn stick(&self, s: Side) -> Stick {
        let a = Self::stick_at(s);
        let mut initial = self.0[a + 2] as i32;
        let e = self.0[a + 10];
        let raw = e & 0x80 != 0;
        let mut max = (e & 0x7F) as i32;
        if max < initial || self.0[a + 1] == 0 {
            initial = 10;
            max = 100;
        }
        Stick { range: Range { initial, max }, raw }
    }

    pub fn set_stick(&mut self, s: Side, v: Stick) {
        let a = Self::stick_at(s);
        self.0[a] = 1;
        self.0[a + 1] = 1;
        self.0[a + 2] = v.range.initial as u8;
        self.0[a + 9] = 1;
        self.0[a + 10] = ((v.range.max & 0x7F) as u8) | if v.raw { 0x80 } else { 0 };
    }

    /// The stick's range as the slider shows it, healing (and storing) a collapsed one.
    pub fn heal_stick(&mut self, s: Side) {
        let mut v = self.stick(s);
        if let Some(r) = heal_range(v.range) {
            v.range = r;
            self.set_stick(s, v);
        }
    }

    pub fn drag_stick(&mut self, s: Side, handle: Handle, pct: i32) {
        self.heal_stick(s);
        let mut v = self.stick(s);
        v.range = drag_range(v.range, handle, pct);
        self.set_stick(s, v);
        self.heal_stick(s);
    }

    pub fn set_radial_trace(&mut self, s: Side, on: bool) {
        let mut v = self.stick(s);
        v.raw = !on;
        self.set_stick(s, v);
    }

    // ---- triggers ----

    fn trigger_at(s: Side) -> usize {
        match s {
            Side::Left => TRIGGERS,
            Side::Right => TRIGGERS + TRIGGER_STRIDE,
        }
    }

    pub fn trigger(&self, s: Side) -> Range {
        let a = Self::trigger_at(s);
        if self.0[a + 1] & 2 != 0 {
            Range { initial: self.0[a + 3] as i32, max: self.0[a + 4] as i32 }
        } else {
            Range { initial: 0, max: 100 }
        }
    }

    pub fn set_trigger(&mut self, s: Side, r: Range) {
        let a = Self::trigger_at(s);
        self.0[a] = 1;
        self.0[a + 1] |= 2;
        self.0[a + 3] = r.initial as u8;
        self.0[a + 4] = r.max as u8;
    }

    pub fn heal_trigger(&mut self, s: Side) {
        if let Some(r) = heal_range(self.trigger(s)) {
            self.set_trigger(s, r);
        }
    }

    pub fn drag_trigger(&mut self, s: Side, handle: Handle, pct: i32) {
        self.heal_trigger(s);
        let r = drag_range(self.trigger(s), handle, pct);
        self.set_trigger(s, r);
        self.heal_trigger(s);
    }

    // ---- D-pad / power ----

    pub fn swap_stick_dpad(&self) -> bool {
        self.0[RESERVE] == 1 && self.0[RESERVE + 1] == 1
    }

    pub fn set_swap_stick_dpad(&mut self, on: bool) {
        self.0[RESERVE] = on as u8;
        self.0[RESERVE + 1] = on as u8;
    }

    pub fn diagonal_lock(&self) -> bool {
        self.0[RESERVE + 4] == 1
    }

    pub fn set_diagonal_lock(&mut self, on: bool) {
        self.0[RESERVE + 4] = on as u8;
    }

    pub fn sleep_minutes(&self) -> u8 {
        self.0[RESERVE + 6]
    }

    pub fn set_sleep_minutes(&mut self, m: u8) {
        self.0[RESERVE + 6] = m;
    }

    pub fn pick_up_to_wake(&self) -> bool {
        self.0[POWER] == 0
    }

    pub fn set_pick_up_to_wake(&mut self, on: bool) {
        self.0[POWER] = if on { 0 } else { 1 };
    }

    // ---- vibration ----

    /// Nearest of the four levels (0..=3) to the stored percentage.
    pub fn vibration_level(&self, s: Side) -> usize {
        let v = self.0[MOTOR + s as usize] as i32;
        let mut best = 0;
        let mut best_d = i32::MAX;
        for (i, &l) in VIBRATION_LEVELS.iter().enumerate() {
            let d = (v - l as i32).abs();
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        best
    }

    pub fn set_vibration_level(&mut self, s: Side, level: usize) {
        self.0[MOTOR + s as usize] = VIBRATION_LEVELS[level.min(3)];
    }

    // ---- gyro ----

    pub fn gyro_method(&self) -> GyroMethod {
        if self.0[SENSOR + 1] == 1 {
            GyroMethod::AlwaysOn
        } else if self.bit(SENSOR, 7) {
            GyroMethod::PressToTurnOff
        } else if self.bit(SENSOR, 1) {
            GyroMethod::PressToTurnOn
        } else {
            GyroMethod::Off
        }
    }

    /// Bit 0 = sensor enabled (always set); bit 1 = press-to-turn-on; bit 7 = press-to-turn-off;
    /// byte 1 = always-on.
    pub fn set_gyro_method(&mut self, m: GyroMethod) {
        self.set_bit(SENSOR, 0, true);
        self.set_bit(SENSOR, 1, m == GyroMethod::PressToTurnOn);
        self.set_bit(SENSOR, 7, m == GyroMethod::PressToTurnOff);
        self.0[SENSOR + 1] = (m == GyroMethod::AlwaysOn) as u8;
    }

    pub fn gyro_output(&self) -> GyroOutput {
        if self.gyro_method() == GyroMethod::Off {
            GyroOutput::Off
        } else if self.0[SENSOR + 9] == 1 {
            GyroOutput::LeftStick
        } else {
            GyroOutput::RightStick
        }
    }

    /// Choosing "Off" turns the gyro off entirely.
    pub fn set_gyro_output(&mut self, o: GyroOutput) {
        match o {
            GyroOutput::Off => self.set_gyro_method(GyroMethod::Off),
            GyroOutput::LeftStick => self.0[SENSOR + 9] = 1,
            GyroOutput::RightStick => self.0[SENSOR + 9] = 0,
        }
    }

    pub fn gyro_button(&self) -> u8 {
        self.0[SENSOR + 2]
    }

    pub fn set_gyro_button(&mut self, code: u8) {
        self.0[SENSOR + 2] = code;
    }

    /// 0 = tilt (default), 1 = rotation. Stored in SENSOR bit 2.
    pub fn gyro_motion_axis(&self) -> u8 {
        self.bit(SENSOR, 2) as u8
    }

    pub fn set_gyro_motion_axis(&mut self, v: u8) {
        self.set_bit(SENSOR, 2, v == 1);
    }

    /// 0 = both, 1 = vertical only, 2 = horizontal only. SENSOR bits 5 and 6.
    pub fn gyro_active_axis(&self) -> u8 {
        match (self.bit(SENSOR, 5), self.bit(SENSOR, 6)) {
            (true, true) => 0,
            (true, false) => 1,
            _ => 2,
        }
    }

    pub fn set_gyro_active_axis(&mut self, v: u8) {
        self.set_bit(SENSOR, 5, v == 0 || v == 1);
        self.set_bit(SENSOR, 6, v == 0 || v == 2);
    }

    pub fn gyro_invert_x(&self) -> bool {
        self.bit(SENSOR, 4)
    }

    pub fn set_gyro_invert_x(&mut self, on: bool) {
        self.set_bit(SENSOR, 4, on);
    }

    pub fn gyro_invert_y(&self) -> bool {
        self.bit(SENSOR, 3)
    }

    pub fn set_gyro_invert_y(&mut self, on: bool) {
        self.set_bit(SENSOR, 3, on);
    }

    pub fn gyro_curve(&self) -> u8 {
        self.0[SENSOR + 3]
    }

    /// Selects a curve; presets also overwrite the four curve bytes, Custom keeps them.
    pub fn set_gyro_curve(&mut self, c: u8) {
        self.0[SENSOR + 3] = c;
        if let Some(p) = CURVE_PRESETS.get(c as usize) {
            self.0[SENSOR + 5..SENSOR + 9].copy_from_slice(p);
        }
    }

    /// (origin1, target1, origin2, target2)
    pub fn gyro_curve_bytes(&self) -> [u8; 4] {
        self.0[SENSOR + 5..SENSOR + 9].try_into().unwrap()
    }

    /// Drag a custom-curve point to chart coordinates (0..=200, y down). No-op unless
    /// the curve is Custom. Keeps origin1 < origin2.
    pub fn drag_gyro_curve(&mut self, point: CurvePoint, sx: f64, sy: f64) {
        if self.0[SENSOR + 3] != CURVE_CUSTOM {
            return;
        }
        let sx = sx.clamp(0.0, 200.0);
        let sy = sy.clamp(0.0, 200.0);
        let mut o = js_round(sx / 200.0 * 255.0) as i32;
        let t = js_round((200.0 - sy) / 200.0 * 127.0) as i32;
        let (o_at, t_at) = match point {
            CurvePoint::P1 => {
                let o2 = self.0[SENSOR + 7];
                o = o.min(if o2 == 0 { 255 } else { o2 as i32 } - 1);
                (SENSOR + 5, SENSOR + 6)
            }
            CurvePoint::P2 => {
                o = o.max(self.0[SENSOR + 5] as i32 + 1);
                (SENSOR + 7, SENSOR + 8)
            }
        };
        self.0[o_at] = o.clamp(0, 255) as u8;
        self.0[t_at] = t.clamp(0, 127) as u8;
    }

    /// Inner = SENSOR+4 (percent), outer = SENSOR+7 scaled from 0..=255. The outer limit
    /// shares its byte with the curve's origin2.
    pub fn gyro_deadzone(&self) -> Range {
        Range {
            initial: self.0[SENSOR + 4] as i32,
            max: js_round(self.0[SENSOR + 7] as f64 / 255.0 * 100.0) as i32,
        }
    }

    pub fn set_gyro_deadzone(&mut self, r: Range) {
        self.0[SENSOR + 4] = r.initial.clamp(0, 100) as u8;
        self.0[SENSOR + 7] = js_round(r.max.clamp(0, 100) as f64 * 255.0 / 100.0) as u8;
    }

    pub fn heal_gyro_deadzone(&mut self) {
        if let Some(r) = heal_range(self.gyro_deadzone()) {
            self.set_gyro_deadzone(r);
        }
    }

    pub fn drag_gyro_deadzone(&mut self, handle: Handle, pct: i32) {
        self.heal_gyro_deadzone();
        let r = drag_range(self.gyro_deadzone(), handle, pct);
        self.set_gyro_deadzone(r);
        self.heal_gyro_deadzone();
    }

    // ---- lighting (only touched by reset) ----

    fn set_light_solid(&mut self, slot: usize, rgb: [u8; 3]) {
        let a = LIGHTS + slot * LIGHT_STRIDE;
        self.0[a] = 1; // keyframe count
        self.0[a + 1] = 5; // effect count, must be 5
        self.0[a + 2] = 20 - 10; // speed 10, stored inverted
        if self.0[a + 3] == 0 {
            self.0[a + 3] = 100; // brightness
        }
        for led in 0..LIGHT_FRAMES * LIGHT_LEDS {
            let p = a + 4 + led * 3;
            self.0[p..p + 3].copy_from_slice(&rgb);
        }
        self.0[LIGHT_INDEX] = slot as u8;
    }

    // ---- reset ----

    /// Factory defaults for mappings, sticks, triggers, vibration, gyro and lighting.
    /// Name, wake, sleep and report rate are left alone.
    pub fn reset_to_defaults(&mut self) {
        for src in MAP_SOURCES {
            self.set_mapping(src, Mapping::Default);
        }
        for s in [Side::Left, Side::Right] {
            self.set_stick(s, Stick { range: Range { initial: 5, max: 100 }, raw: false });
        }
        for s in [Side::Left, Side::Right] {
            self.set_trigger(s, Range { initial: 5, max: 95 });
        }
        self.0[RESERVE + 2] = 0;
        self.0[RESERVE + 3] = 0;
        self.0[MOTOR] = VIBRATION_LEVELS[3];
        self.0[MOTOR + 1] = VIBRATION_LEVELS[3];
        self.set_swap_stick_dpad(false);
        self.set_diagonal_lock(false);
        self.0[SENSOR..SENSOR + 10].copy_from_slice(&[0x61, 0, 0, 2, 0, 0, 0, 255, 127, 0]);
        for slot in 0..4 {
            self.set_light_solid(slot, [0xFF, 0, 0]);
        }
        self.0[LIGHT_INDEX] = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_contiguous() {
        assert_eq!(PADDLES, BUTTONS + 16 * BUTTON_STRIDE);
        assert_eq!(TRIGGERS, PADDLES + 2 * PADDLE_STRIDE);
        assert_eq!(STICKS, TRIGGERS + 2 * TRIGGER_STRIDE);
        assert_eq!(LIGHT_INDEX, STICKS + 2 * STICK_STRIDE);
        assert_eq!(LIGHTS + 4 * LIGHT_STRIDE + 2, PROFILE_LEN);
        assert_eq!(4 + LIGHT_FRAMES * LIGHT_LEDS * 3, LIGHT_STRIDE);
    }

    #[test]
    fn js_round_matches_ties_up() {
        assert_eq!(js_round(76.5), 77.0);
        assert_eq!(js_round(178.5), 179.0);
        assert_eq!(js_round(2.4999), 2.0);
    }
}
