//! Read-only hardware check: connects in dry-run mode (no state-changing report can be
//! sent), reads the firmware version and all four profiles twice, and checks the two
//! reads agree.
//!
//!   cargo run --example probe

use std::time::Duration;

use blitz_config::device::{self, Cmd, Discovery, Event};
use blitz_config::profile::{MAP_SOURCES, Profile, Side};

fn main() {
    let cand = match device::discover().expect("hid") {
        Discovery::Found(c) => c,
        other => panic!("no configurable Blitz: {other:?}"),
    };
    println!("found {} at {}", cand.transport, cand.path.to_string_lossy());
    let h = device::connect(cand, true, || {}).expect("connect");

    let mut reads: Vec<Vec<Box<[u8; 1117]>>> = vec![Vec::new(); 4];
    for round in 0..2 {
        for p in 0..4u8 {
            if !(round == 0 && p == 0) {
                h.send(Cmd::ReadProfile(p)); // profile 1 is read automatically on connect
            }
        }
    }
    let mut pending = 8;
    while pending > 0 {
        match h.events.recv_timeout(Duration::from_secs(10)).expect("timed out") {
            Event::Connected { firmware } => println!("firmware {firmware:?}"),
            Event::ProfileLoaded { profile, data, backup } => {
                println!("profile {} read ok, backup {:?}", profile + 1, backup.unwrap_or_default());
                reads[profile as usize].push(data);
                pending -= 1;
            }
            Event::Error(e) => panic!("{e}"),
            Event::Disconnected(e) => panic!("disconnected: {e:?}"),
            Event::Written { .. } => unreachable!(),
        }
    }
    let live = h.live.lock().unwrap().gamepad;
    println!("live input: {live:?}");
    drop(h);

    for (i, r) in reads.iter().enumerate() {
        assert_eq!(r[0], r[1], "profile {} differs between reads", i + 1);
        let pr = Profile(*r[0]);
        let maps: Vec<_> = MAP_SOURCES.iter().map(|&s| format!("{}→{}", s.label(), pr.mapping(s).label())).collect();
        println!(
            "P{} {:?}: sticks {:?}/{:?} triggers {:?}/{:?} vib {}/{} gyro {:?}/{:?} sleep {} wake {}\n    {}",
            i + 1,
            pr.name(),
            pr.stick(Side::Left),
            pr.stick(Side::Right),
            pr.trigger(Side::Left),
            pr.trigger(Side::Right),
            pr.vibration_level(Side::Left),
            pr.vibration_level(Side::Right),
            pr.gyro_method(),
            pr.gyro_output(),
            pr.sleep_minutes(),
            pr.pick_up_to_wake(),
            maps.join(" "),
        );
    }
    println!("all profiles stable across two reads");
}
