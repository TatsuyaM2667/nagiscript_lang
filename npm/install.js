#!/usr/bin/env node

const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const https = require("https");
const http = require("http");

const REPO = "TatsuyaM2667/nagiscript_lang";
const BIN_NAME = "nagiscript";
const VERSION = require("./package.json").version;

function platform() {
  const p = process.platform;
  if (p === "linux") return "unknown-linux-gnu";
  if (p === "darwin") return "apple-darwin";
  if (p === "win32") return "pc-windows-msvc";
  throw new Error(`Unsupported platform: ${p}`);
}

function arch() {
  const a = process.arch;
  if (a === "x64") return "x86_64";
  if (a === "arm64") return "aarch64";
  throw new Error(`Unsupported arch: ${a}`);
}

function isWin() {
  return process.platform === "win32";
}

function binDir() {
  return path.join(__dirname, "bin");
}

function binPath() {
  const ext = isWin() ? ".exe" : "";
  return path.join(binDir(), BIN_NAME + ext);
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const mod = url.startsWith("https") ? https : http;
    const file = fs.createWriteStream(dest);
    mod
      .get(url, { headers: { "User-Agent": "@nagiscript/cli" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          file.close();
          fs.unlinkSync(dest);
          return download(res.headers.location, dest).then(resolve, reject);
        }
        if (res.statusCode !== 200) {
          file.close();
          fs.unlinkSync(dest);
          return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        }
        res.pipe(file);
        file.on("finish", () => {
          file.close();
          resolve();
        });
      })
      .on("error", (err) => {
        file.close();
        fs.unlinkSync(dest);
        reject(err);
      });
  });
}

async function tryDownloadPrebuilt() {
  const tag = `v${VERSION}`;
  const target = `${arch()}-${platform()}`;
  const ext = isWin() ? ".zip" : ".tar.gz";
  const assetName = `${BIN_NAME}-${tag}-${target}${ext}`;
  const url = `https://github.com/${REPO}/releases/download/${tag}/${assetName}`;

  console.log(`  Trying prebuilt: ${url}`);
  try {
    const tmpDir = path.join(__dirname, ".download-tmp");
    fs.mkdirSync(tmpDir, { recursive: true });
    const tmpFile = path.join(tmpDir, assetName);
    await download(url, tmpFile);

    if (ext === ".tar.gz") {
      execSync(`tar -xzf "${tmpFile}" -C "${binDir()}"`, { stdio: "inherit" });
    } else {
      execSync(`powershell -Command "Expand-Archive -Path '${tmpFile}' -DestinationPath '${binDir()}' -Force"`, { stdio: "inherit" });
    }
    fs.rmSync(tmpDir, { recursive: true, force: true });
    if (!isWin()) fs.chmodSync(binPath(), 0o755);
    return true;
  } catch {
    return false;
  }
}

function cargoBinPath() {
  // Find where cargo installs binaries
  if (isWin()) {
    const home = process.env.USERPROFILE || path.join("C:", "Users", process.env.USERNAME || "");
    return path.join(home, ".cargo", "bin");
  }
  return path.join(process.env.HOME || "/root", ".cargo", "bin");
}

function findCargoBinary() {
  const binDirPath = cargoBinPath();
  const ext = isWin() ? ".exe" : "";
  const candidate = path.join(binDirPath, BIN_NAME + ext);
  if (fs.existsSync(candidate)) return candidate;
  // Fallback: which/where
  try {
    const cmd = isWin() ? `where ${BIN_NAME}` : `which ${BIN_NAME}`;
    return execSync(cmd, { encoding: "utf8" }).trim().split("\n")[0];
  } catch {
    return null;
  }
}

function tryCargoInstall() {
  console.log("  Prebuilt not found. Building from source with cargo...");
  try {
    execSync(`cargo install --git https://github.com/${REPO}.git ngs_driver`, {
      stdio: "inherit",
    });
    const src = findCargoBinary();
    if (!src) {
      console.error("  cargo install succeeded but binary not found");
      return false;
    }
    fs.copyFileSync(src, binPath());
    if (!isWin()) fs.chmodSync(binPath(), 0o755);
    return true;
  } catch {
    return false;
  }
}

async function main() {
  fs.mkdirSync(binDir(), { recursive: true });

  // Check if already installed
  if (fs.existsSync(binPath())) {
    try {
      const out = execSync(`"${binPath()}" --help`, { encoding: "utf8", timeout: 5000 });
      if (out.includes("nagiscript")) {
        console.log(`@nagiscript/cli ${VERSION} already installed.`);
        return;
      }
    } catch {
      // binary is broken, reinstall
    }
  }

  console.log(`Installing @nagiscript/cli ${VERSION}...`);

  if (await tryDownloadPrebuilt()) {
    console.log(`@nagiscript/cli ${VERSION} installed (prebuilt).`);
    return;
  }

  if (tryCargoInstall()) {
    console.log(`@nagiscript/cli ${VERSION} installed (cargo build).`);
    return;
  }

  console.error(
    "\nFailed to install nagiscript. Please install manually:\n" +
      "  cargo install --git https://github.com/" + REPO + ".git ngs_driver\n"
  );
  process.exit(1);
}

main().catch((e) => {
  console.error("Install error:", e.message);
  process.exit(1);
});
