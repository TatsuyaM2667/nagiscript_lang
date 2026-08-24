// NagiScript WASM ホストランナー（Node.js）
// 使い方: node ngs-host.mjs program.wasm [mainExportName]
//
// コンパイラ (nagiscript wasm) が出力する .wasm は "env" モダールからの
// __ngs_* 関数群と、エクスポートされた memory を前提とする。
// このホストはその契約のリファレンス実装であり、JSX 相互運用（props/box）の
// 基礎にもなる（CrValue はホスト側オブジェクトへのハンドルとして扱う）。

import { readFileSync } from "node:fs";
import process from "node:process";

const wasmPath = process.argv[2];
if (!wasmPath) {
  console.error("usage: node ngs-host.mjs <program.wasm> [exportName]");
  process.exit(2);
}
const entryName = process.argv[3] ?? "main";

const bytes = readFileSync(wasmPath);

// --- ボックス / props のホスト側ストレージ ------------------------------
let nextHandle = 1;
const boxes = new Map(); // handle -> {kind:"i64"|"f64"|"bool"|"str"|"obj", value}

function box(kind, value) {
  const h = nextHandle++;
  boxes.set(h, { kind, value });
  return BigInt(h);
}

function unboxStr(h) {
  const b = boxes.get(Number(h));
  if (!b || b.kind !== "str") throw new Error(`bad boxed str handle ${h}`);
  return b.value;
}

// --- メモリアクセス ------------------------------------------------------
let mem = null;
let dv = null;
const enc = new TextEncoder();
const dec = new TextDecoder();

function u8(ptr, len) {
  return new Uint8Array(mem.buffer, ptr, len);
}
function readStrCell(cellPtr) {
  // セルレイアウト: { data: u64 @0, len: u64 @8 }（ネイティブ版と同一）
  const p = dv.getUint32(cellPtr, true);
  const len = Number(dv.getBigUint64(cellPtr + 8, true));
  return dec.decode(u8(p, len));
}
function writeHostString(s) {
  // ホスト管理文字列を wasm メモリにコピーしてセルを作る簡易アロケータ
  if (!hostHeap) hostHeap = Number(instance.exports.__heap_base ?? 0) || heapFallback;
  const b = enc.encode(s);
  const cellLen = 8;
  const total = cellLen + b.length;
  if (hostHeap + total > mem.buffer.byteLength) {
    throw new Error("host arena overflow");
  }
  const cell = hostHeap;
  hostHeap += total;
  dv.setUint32(cell, cell + cellLen, true);
  dv.setUint32(cell + 4, b.length, true);
  new Uint8Array(mem.buffer, cell + cellLen, b.length).set(b);
  return cell;
}
let hostHeap = 0;
let heapFallback = 1024 * 1024;

// --- env 実装 ------------------------------------------------------------
const out = (s) => process.stdout.write(s);
// runtime.c と同じ書式: 整数値は "3.0"、それ以外は %g 相当
function fmtF64(v) {
  if (Number.isFinite(v) && v === Math.trunc(v) && Math.abs(v) < 9.2e18) {
    return v.toFixed(1);
  }
  // %g 相当（有効数字 6 桁、不要な 0 を落とす）
  let s = v.toPrecision(6);
  if (s.includes("e")) return s.replace(/(\.\d*?)0+e/, "$1e").replace(/\.e/, "e");
  return s.replace(/(\.\d*?)0+$/, "$1").replace(/\.0*$/, "");
}
const env = {
  __ngs_print_str: (p, l) => out(dec.decode(u8(p, l))),
  __ngs_println_str: (p, l) => out(dec.decode(u8(p, l)) + "\n"),
  __ngs_print_i64: (v) => out(String(v)),
  __ngs_println_i64: (v) => out(String(v) + "\n"),
  __ngs_print_f64: (v) => out(fmtF64(v)),
  __ngs_println_f64: (v) => out(fmtF64(v) + "\n"),
  __ngs_print_bool: (v) => out(v ? "true" : "false"),
  __ngs_println_bool: (v) => out((v ? "true" : "false") + "\n"),
  __ngs_panic: (p, l) => {
    throw new Error("panic: " + dec.decode(u8(p, l)));
  },
  __ngs_abort: () => {
    throw new Error("abort");
  },
  __ngs_str_eq: (a, b) => (readStrCell(a) === readStrCell(b) ? 1 : 0),
  __ngs_str_to_i64: (cell) => BigInt(Math.trunc(Number(readStrCell(cell)))),
  __ngs_str_to_f64: (cell) => Number(readStrCell(cell)),
  __ngs_box_i64: (v) => box("i64", v),
  __ngs_box_f64: (v) => box("f64", v),
  __ngs_box_bool: (v) => box("bool", v !== 0),
  __ngs_box_str: (cell) => box("str", readStrCell(cell)),
  __ngs_box_ptr: (p) => box("obj", { ptr: p }),
  __ngs_props_new: () => {
    const h = nextHandle++;
    boxes.set(h, { kind: "props", value: { tag: "", props: {}, children: [] } });
    return h;
  },
  __ngs_props_tag: (h, np, nl) => {
    boxes.get(h).value.tag = dec.decode(u8(np, nl));
  },
  __ngs_props_set: (h, np, nl, boxedVal) => {
    const name = dec.decode(u8(np, nl));
    const b = boxes.get(Number(boxedVal));
    boxes.get(h).value.props[name] = b ? b.value : null;
  },
  __ngs_props_add_child: (h, childBoxed) => {
    const c = boxes.get(Number(childBoxed));
    boxes.get(h).value.children.push(c ? c.value : null);
  },
  __ngs_fmod: (a, b) => a % b,
};

// --- 起動 ----------------------------------------------------------------
const mod = await WebAssembly.compile(bytes);
let instance;
try {
  instance = await WebAssembly.instantiate(mod, { env });
} catch (e) {
  console.error("instantiate failed:", e.message);
  process.exit(3);
}
mem = instance.exports.memory;
dv = new DataView(mem.buffer);

const entry = instance.exports[entryName];
if (typeof entry !== "function") {
  console.error(`export \`${entryName}\` not found; have: ${Object.keys(instance.exports).join(", ")}`);
  process.exit(3);
}
try {
  const r = entry();
  process.exit(typeof r === "number" ? r : 0);
} catch (e) {
  console.error(e.message);
  process.exit(1);
}
