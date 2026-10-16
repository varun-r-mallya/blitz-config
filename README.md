# blitz-config

A native configurator for the **Cosmic Byte Blitz** controller, written in Rust with egui.
It's a single ~6 MB binary that links only libc and libudev.

It does what Cosmic Byte's Orbit web configurator does, without a browser or their servers:
- button remapping
- stick and trigger ranges, radial trace, D-pad swap and diagonal lock
- vibration levels and motor tests
- gyro (activation, output, deadzone, response curve, axes, calibration)
- auto-sleep and pick-up-to-wake
- four profiles with names
- a live input preview

> Unofficial. Not affiliated with or endorsed by Cosmic Byte. It writes to your controller's settings; use at your own risk.

## Build and run

    cargo build --release
    ./target/release/blitz-config            # or --dry-run to change nothing

Connect the controller over wired USB (`0281:0080`) or the 2.4 GHz dongle (`0281:0112`), in DInput mode.
Bluetooth and XInput mode don't expose the config channel.
Close any browser tab that is using the controller first: two configurators sending commands at once is unsafe.

On Linux, give your user access to the device once, then replug it:

    sudo cp 70-cosmic-byte-blitz.rules /etc/udev/rules.d/
    sudo udevadm control --reload && sudo udevadm trigger

## How it stays accurate

The controller's settings are a 1117-byte profile written in 55-byte chunks over a vendor HID channel. A wrong byte goes straight into its flash, so:

- **Differential tests against the reference.** `tools/gen-vectors.mjs` runs the Orbit web configurator's own code on a fake DOM and a fake HID device. That covers its profile setters, its slider and curve-drag widgets, its reset handler and its driver.
  It records the bytes and reports they produce into `tests/vectors.json`. `cargo test` requires this implementation to reproduce all of them exactly: 1,000 edit sequences, 250 write plans, and the read, session, rumble, calibrate, profile-switch and firmware-parsing cases.
- **Config channel check.** It only opens a device whose HID report descriptor declares the vendor config collection (usage page `0xFFF0`, OUT report `0x0F`).
- **Complete reads only.** A profile is only editable once all 21 chunks have arrived. Reads (which have no side effects) are retried; writes never are.
- **Backups.** Every profile read is saved to `~/.local/share/blitz-config/backups/` before it can be edited.
- **Read-back after every write.** Each write is read back and compared byte for byte. On a mismatch, auto-apply stops and the editor shows what the controller actually holds.
- **Audit log.** Every report sent and every config reply received is appended to `~/.local/share/blitz-config/hid.log`.
- **Dry run.** `--dry-run` logs writes, rumble, calibration and profile switches instead of sending them.
- **Single session.** Only one `blitz-config` window can be connected at a time.

A read-only hardware check (runs in dry-run): `cargo run --example probe` reads all four profiles twice and checks the reads agree.

### Regenerating the test vectors

The reference web app isn't included. To regenerate or extend the vectors, save your own copy of `blitz-configurator.html` and `BlitzDriver.js` from Orbit into `blitz/`, then:

    node tools/gen-vectors.mjs
    # bigger one-off run:
    VECTORS_CASES=20000 VECTORS_SEED=7 VECTORS_OUT=/tmp/v.json node tools/gen-vectors.mjs
    BLITZ_VECTORS=/tmp/v.json cargo test --release

### Deliberate differences from the web app

- **Collapsed sliders:** if a stored slider range is collapsed (max ≤ initial), the web app pulls the handles apart and saves that as soon as the page is shown.
  This app only does so when you drag that slider, so viewing a page never changes the controller.
- **Pending edits:** they are written before a profile switch or disconnect instead of being dropped.
- **Retries and checks:** reads and session start are retried, writes are read back and verified, and Reset to default asks for confirmation.

## Protocol notes

- **Config commands:** OUT report `0x0F`, padded to 63 bytes: `00 00 3C <opcode> <args…>`.
- **Replies:** IN report `0x10`: `00 00 3C <opcode+1> …`; session commands are answered with `0x06`.
- **Opcodes:**

  | Opcode | Meaning |
  |---|---|
  | `01` | enter config mode |
  | `02` | exit |
  | `03` | write a profile span |
  | `04` | read a profile span |
  | `07` | firmware version |
  | `0C` | apply a written span |
  | `E0` | LED refresh |
  | `FD` | gyro calibration |

- **Profile index:** profiles are numbered 1–4 on the wire. Read replies always carry `01` in byte 5, regardless of which profile was requested.
- **Rumble:** OUT report `0x05`, exactly five bytes: `01 00 00 <right> <left>`.
- **Heartbeat:** `0x0F` `54 34 4B 00`, once a second while connected.
- **Live input:** IN report `0x01`.
- **Profile layout:** see `src/profile.rs`.

## License

MIT
