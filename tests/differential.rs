//! Replays tests/vectors.json (produced by tools/gen-vectors.mjs from the reference web
//! configurator) and requires byte-identical results.

use blitz_config::profile::{Button, CurvePoint, GyroMethod, GyroOutput, Handle, Mapping, Profile, Side};
use blitz_config::protocol::{self as p, PROFILE_LEN, Report};
use serde_json::Value;

fn vectors() -> Value {
    let path = std::env::var("BLITZ_VECTORS")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors.json").to_string());
    let raw = std::fs::read_to_string(path)
        .expect("tests/vectors.json missing; run `node tools/gen-vectors.mjs`");
    serde_json::from_str(&raw).unwrap()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn buf(s: &str) -> [u8; PROFILE_LEN] {
    unhex(s).try_into().unwrap()
}

fn apply_diff(start: &[u8; PROFILE_LEN], d: &Value) -> [u8; PROFILE_LEN] {
    let mut out = *start;
    for e in d.as_array().unwrap() {
        out[e[0].as_u64().unwrap() as usize] = e[1].as_u64().unwrap() as u8;
    }
    out
}

fn reports(v: &Value) -> Vec<Report> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| Report { id: r["id"].as_u64().unwrap() as u8, data: unhex(r["data"].as_str().unwrap()) })
        .collect()
}

fn button(name: &str) -> Button {
    use Button::*;
    match name {
        "Up" => Up, "Down" => Down, "Left" => Left, "Right" => Right, "LB" => Lb, "RB" => Rb,
        "LSB" => Lsb, "RSB" => Rsb, "A" => A, "B" => B, "X" => X, "Y" => Y, "Home" => Home,
        "Select" => Select, "Start" => Start, "Capture" => Capture, "L4" => L4, "R4" => R4,
        "LT" => Lt, "RT" => Rt, other => panic!("unknown button {other}"),
    }
}

fn mapping(name: &str) -> Mapping {
    match name {
        "Default" => Mapping::Default,
        "None" => Mapping::Off,
        b => Mapping::To(button(b)),
    }
}

fn mapping_name(m: Mapping) -> String {
    match m {
        Mapping::Default => "Default".into(),
        Mapping::Off => "None".into(),
        Mapping::To(b) => {
            use Button::*;
            match b {
                Up => "Up", Down => "Down", Left => "Left", Right => "Right", Lb => "LB", Rb => "RB",
                Lsb => "LSB", Rsb => "RSB", A => "A", B => "B", X => "X", Y => "Y", Home => "Home",
                Select => "Select", Start => "Start", Capture => "Capture", L4 => "L4", R4 => "R4",
                Lt => "LT", Rt => "RT",
            }
            .into()
        }
    }
}

fn side(v: &Value) -> Side {
    match v.as_str() {
        Some("L") => Side::Left,
        Some("R") => Side::Right,
        _ => if v.as_u64() == Some(0) { Side::Left } else { Side::Right },
    }
}

fn handle(v: &Value) -> Handle {
    if v.as_u64().unwrap() == 0 { Handle::Initial } else { Handle::Max }
}

fn u(v: &Value) -> u64 {
    v.as_u64().unwrap()
}

fn apply(pr: &mut Profile, o: &Value) {
    let pct = || o["pct"].as_i64().unwrap() as i32;
    match o["op"].as_str().unwrap() {
        "map" => pr.set_mapping(button(o["src"].as_str().unwrap()), mapping(o["target"].as_str().unwrap())),
        "stick_drag" => pr.drag_stick(side(&o["side"]), handle(&o["handle"]), pct()),
        "radial" => pr.set_radial_trace(side(&o["side"]), o["on"].as_bool().unwrap()),
        "trigger_drag" => pr.drag_trigger(side(&o["side"]), handle(&o["handle"]), pct()),
        "swap" => pr.set_swap_stick_dpad(o["on"].as_bool().unwrap()),
        "diag" => pr.set_diagonal_lock(o["on"].as_bool().unwrap()),
        "wake" => pr.set_pick_up_to_wake(o["on"].as_bool().unwrap()),
        "sleep" => pr.set_sleep_minutes(u(&o["min"]) as u8),
        "vibration" => pr.set_vibration_level(side(&o["side"]), u(&o["level"]) as usize),
        "gyro_method" => pr.set_gyro_method(match u(&o["m"]) {
            0 => GyroMethod::Off,
            1 => GyroMethod::PressToTurnOn,
            2 => GyroMethod::AlwaysOn,
            _ => GyroMethod::PressToTurnOff,
        }),
        "gyro_output" => pr.set_gyro_output(match u(&o["o"]) {
            0 => GyroOutput::Off,
            1 => GyroOutput::LeftStick,
            _ => GyroOutput::RightStick,
        }),
        "gyro_button" => pr.set_gyro_button(u(&o["code"]) as u8),
        "gyro_motion_axis" => pr.set_gyro_motion_axis(u(&o["v"]) as u8),
        "gyro_active_axis" => pr.set_gyro_active_axis(u(&o["v"]) as u8),
        "gyro_curve" => pr.set_gyro_curve(u(&o["c"]) as u8),
        "gyro_curve_drag" => pr.drag_gyro_curve(
            if u(&o["point"]) == 1 { CurvePoint::P1 } else { CurvePoint::P2 },
            o["sx"].as_f64().unwrap(),
            o["sy"].as_f64().unwrap(),
        ),
        "gyro_deadzone_drag" => pr.drag_gyro_deadzone(handle(&o["handle"]), pct()),
        "invert_x" => pr.set_gyro_invert_x(o["on"].as_bool().unwrap()),
        "invert_y" => pr.set_gyro_invert_y(o["on"].as_bool().unwrap()),
        "name" => pr.set_name(o["name"].as_str().unwrap()),
        "reset" => pr.reset_to_defaults(),
        other => panic!("unknown op {other}"),
    }
}

fn check_view(pr: &Profile, v: &Value, ctx: &str) {
    for (src, want) in v["maps"].as_object().unwrap() {
        assert_eq!(mapping_name(pr.mapping(button(src))), want.as_str().unwrap(), "{ctx}: map {src}");
    }
    for (i, s) in [Side::Left, Side::Right].into_iter().enumerate() {
        let st = pr.stick(s);
        let w = &v["sticks"][i];
        assert_eq!(
            (st.range.initial as i64, st.range.max as i64, st.raw),
            (w["center"].as_i64().unwrap(), w["edge"].as_i64().unwrap(), w["raw"].as_bool().unwrap()),
            "{ctx}: stick {i}"
        );
        let t = pr.trigger(s);
        let w = &v["triggers"][i];
        assert_eq!((t.initial as i64, t.max as i64), (w["center"].as_i64().unwrap(), w["edge"].as_i64().unwrap()), "{ctx}: trigger {i}");
        assert_eq!(pr.vibration_level(s) as u64, u(&v["vibration"][i]), "{ctx}: vibration {i}");
    }
    assert_eq!(pr.swap_stick_dpad(), v["swap"].as_bool().unwrap(), "{ctx}: swap");
    assert_eq!(pr.diagonal_lock(), v["diag"].as_bool().unwrap(), "{ctx}: diag");
    assert_eq!(pr.sleep_minutes() as u64, u(&v["sleep"]), "{ctx}: sleep");
    assert_eq!(pr.pick_up_to_wake(), v["wake"].as_bool().unwrap(), "{ctx}: wake");
    let g = &v["gyro"];
    assert_eq!(pr.gyro_method() as u64, u(&g["method"]), "{ctx}: gyro method");
    assert_eq!(pr.gyro_output() as u64, u(&g["out"]), "{ctx}: gyro output");
    assert_eq!(pr.gyro_button() as u64, u(&g["btn"]), "{ctx}: gyro button");
    assert_eq!(pr.gyro_curve() as u64, u(&g["curve"]), "{ctx}: gyro curve");
    let dz = pr.gyro_deadzone();
    assert_eq!((dz.initial as u64, dz.max as u64), (u(&g["adz"][0]), u(&g["adz"][1])), "{ctx}: gyro deadzone");
    assert_eq!(pr.gyro_invert_x(), g["invX"].as_bool().unwrap(), "{ctx}: invert x");
    assert_eq!(pr.gyro_invert_y(), g["invY"].as_bool().unwrap(), "{ctx}: invert y");
    assert_eq!(pr.gyro_motion_axis() as u64, u(&g["motionAxis"]), "{ctx}: motion axis");
    assert_eq!(pr.gyro_active_axis() as u64, u(&g["activeAxis"]), "{ctx}: active axis");
    assert_eq!(pr.name(), v["name"].as_str().unwrap(), "{ctx}: name");
}

fn first_diff(a: &[u8], b: &[u8]) -> String {
    match (0..a.len()).find(|&i| a[i] != b[i]) {
        Some(i) => format!("first differing byte {i}: got {:#04x}, want {:#04x}", a[i], b[i]),
        None => "identical".into(),
    }
}

#[test]
fn profile_edits_match_reference() {
    let v = vectors();
    let cases = v["profile_cases"].as_array().unwrap();
    for (n, c) in cases.iter().enumerate() {
        let start = buf(c["start"].as_str().unwrap());
        let want = apply_diff(&start, &c["end_diff"]);
        let mut pr = Profile(start);
        for o in c["ops"].as_array().unwrap() {
            apply(&mut pr, o);
        }
        let ctx = format!("case {n} ({}) ops {}", c["kind"], c["ops"]);
        assert!(pr.0 == want, "{ctx}: {}", first_diff(&pr.0, &want));
        check_view(&pr, &c["view"], &ctx);
    }
    assert!(cases.len() >= 1000);
}

#[test]
fn writes_match_reference() {
    let v = vectors();
    for (n, c) in v["write_cases"].as_array().unwrap().iter().enumerate() {
        let base = buf(c["base"].as_str().unwrap());
        let next = apply_diff(&base, &c["next_diff"]);
        let want = reports(&c["reports"]);
        let got = p::write_plan(u(&c["profile"]) as u8, &next, &base).map(|w| w.reports).unwrap_or_default();
        assert_eq!(got.len(), want.len(), "write case {n}: report count");
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(g, w, "write case {n}: report {i}");
        }
    }
}

#[test]
fn reads_match_reference() {
    let v = vectors();
    for c in v["read_cases"].as_array().unwrap() {
        let profile = u(&c["profile"]) as u8;
        let want = reports(&c["reports"]);
        assert_eq!(p::read_requests(profile), want, "read requests for profile {profile}");

        // Feed the replies the fake device produced and check assembly.
        let source = unhex(c["source"].as_str().unwrap());
        let mut asm = p::ProfileAssembler::default();
        for (addr, len) in p::profile_chunks().collect::<Vec<_>>().into_iter().rev() {
            let mut wire = vec![0x10, 0x00, 0x00, 0x3C, 0x05, profile + 1, (addr >> 8) as u8, addr as u8, len as u8];
            wire.extend_from_slice(&source[addr..addr + len]);
            wire.resize(64, 0);
            assert_eq!(asm.accept(&wire), p::ChunkResult::Accepted);
        }
        assert_eq!(asm.finish().unwrap().to_vec(), unhex(c["assembled"].as_str().unwrap()));
    }
}

#[test]
fn session_and_misc_reports_match_reference() {
    let v = vectors();
    for c in v["misc"].as_array().unwrap() {
        let a = c["args"].as_array().unwrap();
        let arg = |i: usize| u(&a[i]) as u8;
        let got: Vec<Report> = match c["call"].as_str().unwrap() {
            "switch_profile" => vec![p::switch_profile(arg(0))],
            "rumble" => vec![p::rumble(arg(0), arg(1))],
            "calibrate" => vec![p::calibrate_gyro()],
            "heartbeat" => vec![p::heartbeat()],
            "set_light" => vec![p::set_light(arg(0) == 1, arg(1))],
            "enter" => vec![p::enter()],
            "exit" => vec![p::exit(arg(0) == 1)],
            "firmware_request" => vec![p::firmware_request()],
            "vib_pwm" => {
                let lvl = u(&a[0]) as f64;
                let pwm = blitz_config::profile::js_round(lvl * 255.0 / 100.0) as u64;
                assert_eq!(pwm, u(&c["result"]), "vibration pwm for {lvl}");
                continue;
            }
            other => panic!("unknown call {other}"),
        };
        assert_eq!(got, reports(&c["reports"]), "{}", c["call"]);
    }
}

#[test]
fn firmware_decoding_matches_reference() {
    let v = vectors();
    for c in v["fw_cases"].as_array().unwrap() {
        let wire = unhex(c["wire"].as_str().unwrap());
        let decoded = p::decode_firmware(&wire);
        assert_eq!(decoded.as_deref(), c["decoded"].as_str());
        assert_eq!(decoded.map(|d| p::normalize_firmware(&d)).as_deref(), c["normalized"].as_str());
    }
    for c in v["version_cases"].as_array().unwrap() {
        let (l, i) = (c["latest"].as_str().unwrap(), c["installed"].as_str().unwrap());
        assert_eq!(p::firmware_is_newer(l, i), c["newer"].as_bool().unwrap(), "{l} vs {i}");
    }
}
