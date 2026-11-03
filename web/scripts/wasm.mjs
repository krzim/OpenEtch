#!/usr/bin/env node
// Build the browser's WebAssembly core (crates/webcore) into web/src/wasm/pkg, where the preview worker
// (src/wasm/previewWorker.ts) imports it. `npm run build` and `npm run dev` run this first.
//
//   node scripts/wasm.mjs           release: cargo --profile wasm-release, wasm-bindgen, then wasm-opt -Oz
//   node scripts/wasm.mjs --dev     quick: cargo's dev profile, no wasm-opt (for `npm run dev`)
//   node scripts/wasm.mjs --tools   install the pinned tools into <repo>/target/tools (once per checkout)
//   node scripts/wasm.mjs --design  the design-only build's core (webcore feature `design`: the document API in the
//                                   browser), cargo --profile wasm-design (sized for the download), into
//                                   web/src/standalone/pkg; with --dev, cargo's dev profile
//
// Tools, all pinned:
// - wasm-bindgen-cli: exactly the wasm-bindgen version in Cargo.lock (the generated glue must match the crate).
//   Found as $WASM_BINDGEN, else <repo>/target/tools/bin/wasm-bindgen, else on PATH; another version is an error.
// - binaryen's wasm-opt, release BINARYEN below (Apache-2.0, build time only): $WASM_OPT (a native wasm-opt, or
//   binaryen's wasm-opt.js), else <repo>/target/tools/binaryen-<release>/wasm-opt.js (binaryen's Node build, run with
//   this Node; the download is checked against BINARYEN_SHA256), else wasm-opt on PATH. Fallback when none is there
//   (offline, first checkout): the wasm is used as wasm-bindgen wrote it, a warning says so, and it works the same;
//   only the size optimisation is skipped. OPENETCH_REQUIRE_WASM_OPT=1 turns that into an error (release builds).
//
// Nothing here is a runtime dependency: the output is plain JS glue plus one .wasm file.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const web = resolve(here, "..");
const repo = resolve(web, "..");
const tools = join(repo, "target", "tools");
const argv = process.argv.slice(2);
const dev = argv.includes("--dev");
const design = argv.includes("--design");
const out = design ? join(web, "src", "standalone", "pkg") : join(web, "src", "wasm", "pkg");
const exe = process.platform === "win32" ? ".exe" : "";

const BINARYEN = "version_133";
const BINARYEN_SHA256 = "3507aedecef25c46f2889530a7da304677e97122869274125f782b586cb508ab";
const WASM_OPT_FLAGS = [
  "-Oz",
  // What rustc's wasm32-unknown-unknown target emits by default.
  "--enable-bulk-memory",
  "--enable-nontrapping-float-to-int",
  "--enable-sign-ext",
  "--enable-mutable-globals",
  "--enable-reference-types",
  "--enable-multivalue",
  // No names or producer sections in the shipped file.
  "--strip-debug",
  "--strip-producers",
];

function log(msg) {
  console.log(`[wasm] ${msg}`);
}

function run(cmd, args, opts = {}) {
  execFileSync(cmd, args, { stdio: "inherit", ...opts });
}

/** The wasm-bindgen version Cargo.lock pins. */
function lockedBindgen() {
  const lock = readFileSync(join(repo, "Cargo.lock"), "utf8");
  const m = /\[\[package\]\]\r?\nname = "wasm-bindgen"\r?\nversion = "([^"]+)"/.exec(lock);
  if (!m) throw new Error("Cargo.lock has no wasm-bindgen package");
  return m[1];
}

function versionOf(bin) {
  try {
    return execFileSync(bin, ["--version"], { encoding: "utf8" }).trim().split(/\s+/).pop();
  } catch {
    return null;
  }
}

function findBindgen(want) {
  const candidates = [process.env.WASM_BINDGEN, join(tools, "bin", `wasm-bindgen${exe}`), "wasm-bindgen"].filter(Boolean);
  const seen = [];
  for (const c of candidates) {
    const v = versionOf(c);
    if (v === want) return c;
    if (v) seen.push(`${c} is ${v}`);
  }
  throw new Error(
    `wasm-bindgen ${want} (the version in Cargo.lock) was not found${seen.length ? ` (${seen.join("; ")})` : ""}. ` +
      "Run `npm run wasm:tools` once, or set WASM_BINDGEN.",
  );
}

/** [command, leading args] for wasm-opt, or null. */
function findWasmOpt() {
  const env = process.env.WASM_OPT;
  if (env) return env.endsWith(".js") ? [process.execPath, [env]] : [env, []];
  const js = join(tools, `binaryen-${BINARYEN}`, "wasm-opt.js");
  if (existsSync(js)) return [process.execPath, [js]];
  if (versionOf("wasm-opt")) return ["wasm-opt", []];
  return null;
}

async function installTools() {
  mkdirSync(tools, { recursive: true });
  const want = lockedBindgen();
  if (versionOf(join(tools, "bin", `wasm-bindgen${exe}`)) === want) log(`wasm-bindgen ${want} already installed`);
  else run("cargo", ["install", "wasm-bindgen-cli", "--version", want, "--locked", "--root", tools]);
  const dir = join(tools, `binaryen-${BINARYEN}`);
  if (existsSync(join(dir, "wasm-opt.js"))) {
    log(`binaryen ${BINARYEN} already installed`);
    return;
  }
  const url = `https://github.com/WebAssembly/binaryen/releases/download/${BINARYEN}/binaryen-${BINARYEN}-node.tar.gz`;
  log(`downloading ${url}`);
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  const bytes = Buffer.from(await res.arrayBuffer());
  const sha = createHash("sha256").update(bytes).digest("hex");
  if (sha !== BINARYEN_SHA256) throw new Error(`binaryen download checksum ${sha}, expected ${BINARYEN_SHA256}`);
  const tgz = join(tools, `binaryen-${BINARYEN}-node.tar.gz`);
  writeFileSync(tgz, bytes);
  // tar ships with Windows 10+, macOS and Linux. Run it in the tools folder with a bare file name: GNU tar (Git Bash's,
  // first on PATH there) reads "C:\..." as host:path and fails.
  run("tar", ["-xzf", basename(tgz)], { cwd: tools });
  rmSync(tgz);
  if (!existsSync(join(dir, "wasm-opt.js"))) throw new Error(`no wasm-opt.js in the binaryen ${BINARYEN} archive`);
  log(`binaryen ${BINARYEN} installed in ${dir}`);
}

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function build() {
  const want = lockedBindgen();
  const bindgen = findBindgen(want);
  const profile = dev ? "dev" : design ? "wasm-design" : "wasm-release";
  const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(repo, "target");
  log(`cargo build -p webcore (${profile}${design ? ", feature design" : ""})`);
  const features = design ? ["--features", "design"] : [];
  run("cargo", ["build", "-p", "webcore", "--target", "wasm32-unknown-unknown", "--profile", profile, "--locked", ...features], { cwd: repo });
  const built = join(targetDir, "wasm32-unknown-unknown", dev ? "debug" : profile, "webcore.wasm");
  // Unchanged input and tools: keep the last output (cargo already did nothing).
  const stampFile = join(out, ".stamp");
  const stamp = JSON.stringify({ input: sha256(built), bindgen: want, dev, design, binaryen: BINARYEN });
  if (existsSync(stampFile) && readFileSync(stampFile, "utf8") === stamp && existsSync(join(out, "webcore_bg.wasm"))) {
    log("up to date");
    return;
  }
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  run(bindgen, ["--target", "web", "--no-typescript", "--out-dir", out, "--out-name", "webcore", built]);
  const wasm = join(out, "webcore_bg.wasm");
  let optimized = false;
  if (!dev) {
    const opt = findWasmOpt();
    if (opt) {
      const tmp = `${wasm}.opt`;
      const before = statSync(wasm).size;
      log(`wasm-opt ${WASM_OPT_FLAGS.join(" ")}`);
      run(opt[0], [...opt[1], wasm, ...WASM_OPT_FLAGS, "-o", tmp]);
      copyFileSync(tmp, wasm);
      rmSync(tmp);
      optimized = true;
      log(`wasm-opt: ${before} -> ${statSync(wasm).size} bytes`);
    } else if (process.env.OPENETCH_REQUIRE_WASM_OPT === "1") {
      throw new Error("wasm-opt not found (OPENETCH_REQUIRE_WASM_OPT=1). Run `npm run wasm:tools`, or set WASM_OPT.");
    } else {
      console.warn("[wasm] warning: wasm-opt not found, the wasm is not size-optimised (run `npm run wasm:tools` once).");
    }
  }
  writeFileSync(stampFile, stamp);
  log(`${wasm}: ${statSync(wasm).size} bytes${optimized ? " (wasm-opt -Oz)" : ""}`);
}

try {
  if (argv.includes("--tools")) await installTools();
  else build();
} catch (e) {
  console.error(`[wasm] ${e instanceof Error ? e.message : String(e)}`);
  process.exit(1);
}
