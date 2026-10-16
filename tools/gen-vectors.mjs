// Generates tests/vectors.json by running the reference Orbit web configurator as an oracle.
// The reference is not part of this repo: save your own copy of
// orbit.thecosmicbyte.com/blitz-configurator.html and its BlitzDriver.js into blitz/
// (the committed vectors were generated from Orbit v2.93.0).
//
// Nothing here re-implements the reference logic: profile edits go through the page's
// own functions (lifted out of its inline <script>), sliders through its own dualRange()
// widget on a fake DOM, the custom-curve drag through its own handler, "Reset to default"
// through its own click handler, and every report through the real BlitzDriver against
// a fake HID device. The Rust tests must reproduce all of it byte-for-byte.
//
//   node tools/gen-vectors.mjs            # writes tests/vectors.json
//   VECTORS_CASES=20000 VECTORS_SEED=7 VECTORS_OUT=/tmp/v.json node tools/gen-vectors.mjs
//                                         # bigger one-off run (point BLITZ_VECTORS at it)

import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const html = fs.readFileSync(path.join(root, 'blitz/blitz-configurator.html'), 'utf8');
const driverSrc = fs.readFileSync(path.join(root, 'blitz/BlitzDriver.js'), 'utf8');

// ---------------- deterministic RNG ----------------
let seed = Number(process.env.VECTORS_SEED || 0x5eed1234);
const rnd = () => {
  seed = (seed + 0x6d2b79f5) | 0;
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const ri = (lo, hi) => lo + Math.floor(rnd() * (hi - lo + 1));
const pick = (a) => a[ri(0, a.length - 1)];
const hex = (u8) => Buffer.from(u8).toString('hex');
// [[index, value], ...] where `b` differs from `a`
const diff = (a, b) => { const d = []; for (let i = 0; i < b.length; i++) if (a[i] !== b[i]) d.push([i, b[i]]); return d; };

// ---------------- reference driver in a sandbox ----------------
// Timers fire immediately so the driver's pacing sleeps don't slow generation down;
// pacing isn't part of what's compared.
const sandbox = { console, Promise, Uint8Array, Array, Set, Math, Date, Error };
sandbox.setTimeout = (fn) => { setImmediate(fn); return 0; };
sandbox.clearTimeout = () => {};
sandbox.setInterval = () => 0;
sandbox.clearInterval = () => {};
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(driverSrc, sandbox);
const { BlitzDriver, BlitzProto } = sandbox;
const { OFF, VIB_LEVELS } = BlitzProto;

class FakeHid {
  constructor() { this.sent = []; this.opened = true; this.productId = 0x0080; this.listeners = []; this.onSend = null; }
  async open() {}
  async close() {}
  addEventListener(_t, fn) { this.listeners.push(fn); }
  removeEventListener() {}
  async sendReport(id, data) {
    this.sent.push({ id, data: hex(data) });
    if (this.onSend) this.onSend(id, Uint8Array.from(data));
  }
  emit(reportId, payload) {
    const u8 = Uint8Array.from(payload);
    for (const fn of this.listeners) fn({ reportId, data: new DataView(u8.buffer) });
  }
}

// ---------------- reference page logic, lifted from its inline script ----------------
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
const main = scripts.find((s) => s.includes('const BTN='));
const between = (s, a, b) => {
  const i = s.indexOf(a), j = s.indexOf(b, i);
  if (i < 0 || j < 0) throw new Error('marker not found: ' + a + ' / ' + b);
  return s.slice(i, j);
};
const core = between(main, 'const bit=', 'const MAP_TARGETS=');
const names = between(main, 'function readName(', 'function syncProfileHeader(');
const fw = between(main, 'function fwNormalize(', 'function fwSetBadge(');
const resetBody = between(main, "$('#btnReset').onclick=()=>{if(!buf)return;", "$$('.nav-item')")
  .replace("$('#btnReset').onclick=", 'const doReset=');
const curveDrag = between(main, '(function wireCurveDrag(){', '/* ============ LIVE PREVIEW');

// The map rows the reset handler iterates, in page order.
const mapRows = [...html.matchAll(/class="map-row[^"]*"[^>]*data-src="([A-Za-z]+)"/g)].map((m) => m[1]);
if (mapRows.length !== 18) throw new Error('expected 18 map rows, got ' + mapRows.length);

// Minimal DOM: enough for dualRange() and the curve-drag handler to run unmodified.
function makeDom() {
  const docListeners = {};
  const el = (extra = {}) => ({ style: { setProperty() {} }, classList: { add() {}, remove() {}, toggle() {} }, setAttribute() {}, addEventListener(t, fn) { (this._l ||= {})[t] = fn; }, ...extra });
  const elements = { gCurvP1: el(), gCurvP2: el() };
  const svg = el({ getBoundingClientRect: () => ({ left: 0, top: 0, width: 200, height: 200 }) });
  const document = {
    addEventListener(t, fn) { docListeners[t] = fn; },
    removeEventListener(t) { delete docListeners[t]; },
    getElementById: (id) => elements[id] || null,
    querySelector: () => null,
  };
  const makeHost = () => {
    const track = el({ getBoundingClientRect: () => ({ left: 0, width: 100 }) });
    const fill = el(), hs = [el(), el()];
    return { set innerHTML(_v) {}, querySelector: (s) => (s === '.track' ? track : fill), querySelectorAll: () => hs, hs };
  };
  return { document, docListeners, elements, svg, makeHost };
}

const dom = makeDom();
const factory = new Function('OFF', 'VIB_LEVELS', 'document', '$', '$$', 'mapRows', `
  "use strict";
  let buf=null, curAnim=0, curKeyframe=0;
  const markDirty=()=>{}, applyUI=()=>{}, refreshVals=()=>{};
  ${core}
  ${names}
  ${fw}
  ${resetBody}
  ${curveDrag}
  return {
    set buf(b){buf=b;}, get buf(){return buf;},
    readMap, writeMap, readStick, writeStick, readTrig, writeTrig, readSwap, writeSwap, readDiag, writeDiag,
    readStandby, writeStandby, readWake, writeWake, lvlToIdx, idxToLvl, vibPWM, readMotion, readMotionMethod,
    writeMotionMethod, writeMotionOutput, readMotionAxis, writeMotionAxis, readActiveAxis, writeActiveAxis,
    applyCurve, writeMotionAdz, setBit, readName, writeName, dualRange, doReset, fwNormalize, fwIsNewer,
    SEN: OFF.SENSOR.at,
  };
`);
const $ = (sel) => (sel === '#gCurvChart' ? dom.svg : null);
const $$ = (sel) => (sel === '.map-row[data-src]' ? mapRows.map((src) => ({ dataset: { src } })) : []);
const P = factory(OFF, VIB_LEVELS, dom.document, $, $$, mapRows);

// Drag one handle of a dualRange widget built exactly as applyUI() builds it.
function dragRange(get, set, handle, clientX) {
  const host = dom.makeHost();
  P.dualRange(host, get, set); // construction repaints, which may self-heal a collapsed range
  host.hs[handle].onmousedown({ preventDefault() {} });
  dom.docListeners.mousemove({ clientX });
  dom.docListeners.mouseup();
}
const stickRange = (s) => [() => P.readStick(s), (v) => P.writeStick(s, Object.assign(P.readStick(s), v))];
const trigRange = (t) => [() => P.readTrig(t), (v) => P.writeTrig(t, Object.assign(P.readTrig(t), v))];
const motionRange = () => [() => ({ center: P.readMotion().adzIn, edge: P.readMotion().adzOut }), (v) => P.writeMotionAdz(v)];

function dragCurve(point, clientX, clientY) {
  const start = dom.elements[point === 1 ? 'gCurvP1' : 'gCurvP2']._l.mousedown;
  start({ preventDefault() {} });
  if (dom.docListeners.mousemove) {
    dom.docListeners.mousemove({ clientX, clientY });
    dom.docListeners.mouseup();
  }
}

// ---------------- profile edit vectors ----------------
const SIDES = ['L', 'R'];
const TARGETS = ['Default', 'None', 'Up', 'Down', 'Left', 'Right', 'A', 'B', 'X', 'Y', 'LB', 'RB', 'LT', 'RT', 'LSB', 'RSB', 'Home', 'Select', 'Start', 'Capture'];
const GYRO_BTNS = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 19, 20, 14, 15];

// Each op: generate args, apply through the reference page, return the args for Rust.
const OPS = {
  map: () => { const a = { src: pick(mapRows), target: pick(TARGETS) }; P.writeMap(a.src, a.target); return a; },
  stick_drag: () => {
    const a = { side: pick(SIDES), handle: ri(0, 1), x: ri(-15, 115) };
    dragRange(...stickRange(a.side), a.handle, a.x);
    return a;
  },
  radial: () => { const a = { side: pick(SIDES), on: rnd() < 0.5 }; const v = P.readStick(a.side); v.raw = !a.on; P.writeStick(a.side, v); return a; },
  trigger_drag: () => {
    const a = { side: pick(SIDES), handle: ri(0, 1), x: ri(-15, 115) };
    dragRange(...trigRange(a.side), a.handle, a.x);
    return a;
  },
  swap: () => { const a = { on: rnd() < 0.5 }; P.writeSwap(a.on); return a; },
  diag: () => { const a = { on: rnd() < 0.5 }; P.writeDiag(a.on); return a; },
  wake: () => { const a = { on: rnd() < 0.5 }; P.writeWake(a.on); return a; },
  sleep: () => { const a = { min: pick([0, 1, 5, 10]) }; P.writeStandby(a.min); return a; },
  vibration: () => { const a = { side: ri(0, 1), level: ri(0, 3) }; P.buf[OFF.MOTOR.at + a.side] = P.idxToLvl(a.level); return a; },
  gyro_method: () => { const a = { m: pick([0, 1, 2, 3]) }; P.writeMotionMethod(a.m); return a; },
  gyro_output: () => { const a = { o: ri(0, 2) }; P.writeMotionOutput(a.o); return a; },
  gyro_button: () => { const a = { code: pick(GYRO_BTNS) }; P.buf[P.SEN + 2] = a.code; return a; },
  gyro_motion_axis: () => { const a = { v: ri(0, 1) }; P.writeMotionAxis(a.v); return a; },
  gyro_active_axis: () => { const a = { v: ri(0, 2) }; P.writeActiveAxis(a.v); return a; },
  gyro_curve: () => { const a = { c: ri(0, 3) }; P.applyCurve(a.c); return a; },
  gyro_curve_drag: () => {
    const cx = ri(-30, 230) + rnd(), cy = ri(-30, 230) + rnd();
    // the handler's own pointer -> chart mapping, recorded so Rust gets the same doubles
    const a = { point: ri(1, 2), sx: (cx - 0) / 200 * 200, sy: (cy - 0) / 200 * 200 };
    dragCurve(a.point, cx, cy);
    return a;
  },
  gyro_deadzone_drag: () => {
    const a = { handle: ri(0, 1), x: ri(-15, 115) };
    dragRange(...motionRange(), a.handle, a.x);
    return a;
  },
  invert_x: () => { const a = { on: rnd() < 0.5 }; P.setBit(P.SEN, 4, a.on); return a; },
  invert_y: () => { const a = { on: rnd() < 0.5 }; P.setBit(P.SEN, 3, a.on); return a; },
  name: () => {
    const pool = ['', 'FPS', 'Racing 2', 'a'.repeat(30), 'Ünïcødé 🎮 profile', '日本語のプロファイル名前です長い'];
    const a = { name: pick(pool) + (rnd() < 0.3 ? String(ri(0, 999)) : '') };
    P.writeName(a.name);
    return a;
  },
  reset: () => { P.doReset(); return {}; },
};
// The dualRange pointer mapping, for the record: Math.round((x - left) / width * 100) clamped.
const pct = (x) => Math.max(0, Math.min(100, Math.round((x - 0) / 100 * 100)));

function view() {
  const m = P.readMotion();
  return {
    maps: Object.fromEntries(mapRows.map((s) => [s, P.readMap(s)])),
    sticks: SIDES.map((s) => P.readStick(s)),
    triggers: SIDES.map((t) => P.readTrig(t)),
    swap: P.readSwap(), diag: P.readDiag(), sleep: P.readStandby(), wake: P.readWake(),
    vibration: [P.lvlToIdx(P.buf[OFF.MOTOR.at]), P.lvlToIdx(P.buf[OFF.MOTOR.at + 1])],
    gyro: { method: m.method, out: m.out, btn: m.btn, curve: m.resp, adz: [m.adzIn, m.adzOut], invX: m.invX, invY: m.invY, motionAxis: P.readMotionAxis(), activeAxis: P.readActiveAxis() },
    name: P.readName(),
  };
}

function startBuffer(kind) {
  const b = new Uint8Array(OFF.TAIL.at + OFF.TAIL.len);
  if (kind === 'random') for (let i = 0; i < b.length; i++) b[i] = ri(0, 255);
  if (kind === 'sparse') for (let i = 0; i < b.length; i++) b[i] = rnd() < 0.15 ? ri(0, 255) : 0;
  if (kind === 'plausible') {
    // valid-looking ranges so the non-fallback branches get exercised too
    for (let i = 0; i < b.length; i++) b[i] = rnd() < 0.5 ? ri(0, 3) : 0;
    for (const a of [OFF.STICKS.at, OFF.STICKS.at + 16]) { b[a + 1] = 1; b[a + 2] = ri(0, 40); b[a + 10] = ri(50, 100) | (rnd() < 0.5 ? 0x80 : 0); }
    for (const a of [OFF.TRIGGERS.at, OFF.TRIGGERS.at + 11]) { b[a + 1] = 2; b[a + 3] = ri(0, 40); b[a + 4] = ri(50, 100); }
  }
  if (kind === 'collapsed') {
    // equal / adjacent handles, which trigger dualRange's self-heal
    for (const a of [OFF.STICKS.at, OFF.STICKS.at + 16]) { const v = ri(0, 100); b[a + 1] = 1; b[a + 2] = v; b[a + 10] = v; }
    for (const a of [OFF.TRIGGERS.at, OFF.TRIGGERS.at + 11]) { const v = ri(0, 100); b[a + 1] = 2; b[a + 3] = v; b[a + 4] = v; }
    b[OFF.SENSOR.at + 4] = ri(0, 100); b[OFF.SENSOR.at + 7] = ri(0, 255);
  }
  return b;
}

const opNames = Object.keys(OPS);
const profileCases = [];
const PROFILE_CASES = Number(process.env.VECTORS_CASES || 1000);
for (let n = 0; n < PROFILE_CASES; n++) {
  const kind = pick(['random', 'zero', 'sparse', 'plausible', 'collapsed']);
  P.buf = startBuffer(kind);
  const startBuf = P.buf.slice();
  const start = hex(startBuf);
  const ops = [];
  const count = n < 600 ? 1 : ri(2, 12); // first 600: one of each op in turn
  for (let k = 0; k < count; k++) {
    const op = n < 600 ? opNames[n % opNames.length] : pick(opNames);
    ops.push({ op, ...OPS[op]() });
  }
  profileCases.push({ kind, start, ops, end_diff: diff(startBuf, P.buf), view: view() });
}
for (const c of profileCases) for (const o of c.ops) if ('x' in o) o.pct = pct(o.x);

// ---------------- report vectors from the real driver ----------------
async function driverCase(fn) {
  const dev = new FakeHid();
  const drv = new BlitzDriver(dev);
  await drv.open();
  await fn(drv, dev);
  return dev.sent;
}

const writeCases = [];
for (let n = 0; n < 250; n++) {
  const len = OFF.TAIL.at + OFF.TAIL.len;
  const base = startBuffer(pick(['random', 'zero']));
  const next = base.slice();
  const shape = n % 5;
  if (shape === 0) next[ri(0, len - 1)] ^= 0xff;                               // single byte
  else if (shape === 1) { const a = ri(0, len - 1), b = ri(a, Math.min(len - 1, a + ri(0, 200))); for (let i = a; i <= b; i++) next[i] = ri(0, 255); }
  else if (shape === 2) for (let i = 0; i < len; i++) next[i] = ri(0, 255);    // everything
  else if (shape === 3) { next[ri(0, 426)] ^= 1; next[ri(427, len - 1)] ^= 1; } // spans into lighting
  // shape 4: no change
  const profile = ri(0, 3);
  const baseCopy = base.slice();
  const reports = await driverCase(async (drv) => {
    // the page's autoApply(): diff span, writeDiff, then LED refresh if lighting was touched
    let e = -1; for (let i = 0; i < next.length; i++) if (next[i] !== baseCopy[i]) e = i;
    if (e < 0) return;
    await drv.writeDiff(next, profile, baseCopy);
    if (e >= OFF.LIGHT.at) await drv.setLight(1, 0);
  });
  writeCases.push({ profile, base: hex(base), next_diff: diff(base, next), reports });
}

const readCases = [];
for (let profile = 0; profile < 4; profile++) {
  const src = startBuffer('random');
  let assembled = null;
  const reports = await driverCase(async (drv, dev) => {
    const replies = [];
    dev.onSend = (id, d) => {
      if (id !== 0x0f || d[3] !== 0x04) return;
      const addr = (d[5] << 8) | d[6], n = d[7];
      const r = new Uint8Array(63); r.set([0x00, 0x00, 0x3c, 0x05, d[4], d[5], d[6], n]); r.set(src.slice(addr, addr + n), 8);
      replies.push(r);
      dev.emit(0x10, r);
    };
    assembled = await drv.read(profile);
  });
  readCases.push({ profile, source: hex(src), assembled: hex(assembled), reports });
}

const misc = [];
for (let p = 0; p < 4; p++) misc.push({ call: 'switch_profile', args: [p], reports: await driverCase((d) => d.switchProfile(p)) });
for (const [l, r] of [[0, 0], [255, 0], [0, 255], [255, 255], [77, 179], [12, 200]]) misc.push({ call: 'rumble', args: [l, r], reports: await driverCase((d) => d.vibrate(l, r, 0)) });
misc.push({ call: 'calibrate', args: [], reports: await driverCase((d) => d.calibrate(1)) });
misc.push({ call: 'heartbeat', args: [], reports: await driverCase((d) => d.heartbeat(0)) });
misc.push({ call: 'set_light', args: [1, 0], reports: await driverCase((d) => d.setLight(1, 0)) });
misc.push({ call: 'enter', args: [], reports: await driverCase((d) => d.enter()) });
misc.push({ call: 'exit', args: [0], reports: await driverCase((d) => d.exitSave(false)) });
misc.push({ call: 'firmware_request', args: [], reports: await driverCase((d) => d.fw()) });
for (let lv = 0; lv <= 3; lv++) misc.push({ call: 'vib_pwm', args: [P.idxToLvl(lv)], result: P.vibPWM(P.idxToLvl(lv)) });

const fwCases = [];
const drv = new BlitzDriver(new FakeHid());
for (const digits of [[0x30, 0, 0x32, 0, 0x31, 0, 0x32, 0], [0x30, 0x2e, 0x32, 0x2e, 0x31, 0x2e, 0x32, 0x2e], [0x32, 0x31, 0x32, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0, 0, 0], [0x31, 0x39, 0xff, 0x30, 0x41, 0x35, 0x00, 0x38]]) {
  const wire = [0x10, 0x00, 0x00, 0x3c, 0x08, ...digits, ...new Array(51).fill(0)];
  const decoded = drv._decodeFw(wire);
  fwCases.push({ wire: hex(wire), decoded, normalized: decoded == null ? null : P.fwNormalize(decoded) });
}
const versionCases = [];
for (const latest of ['2.1.2', '2.1.10', '3.0']) for (const installed of ['0.2.1.2', '2.1.2', '2.1.1', '0.2.1.11', '1.9', '0.3.0.0', '2.2'])
  versionCases.push({ latest, installed, newer: P.fwIsNewer(latest, installed) });

const out = { generated_from: 'blitz/ Orbit v2.93.0', profile_cases: profileCases, write_cases: writeCases, read_cases: readCases, misc, fw_cases: fwCases, version_cases: versionCases };
fs.mkdirSync(path.join(root, 'tests'), { recursive: true });
fs.writeFileSync(process.env.VECTORS_OUT || path.join(root, 'tests/vectors.json'), JSON.stringify(out));
const nOps = profileCases.reduce((s, c) => s + c.ops.length, 0);
console.log(`profile cases ${profileCases.length} (${nOps} ops), write ${writeCases.length}, read ${readCases.length}, misc ${misc.length}, fw ${fwCases.length}, versions ${versionCases.length}`);
