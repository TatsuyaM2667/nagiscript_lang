#!/usr/bin/env node

const { execFileSync } = require("child_process");
const path = require("path");
const fs = require("fs");

const binDir = path.join(__dirname);
const ext = process.platform === "win32" ? ".exe" : "";
const bin = path.join(binDir, "nagiscript" + ext);

if (!fs.existsSync(bin)) {
  console.error(
    "nagiscript binary not found. Run: npm install @nagiscript/cli\n" +
      "Or install manually: cargo install --git https://github.com/TatsuyaM2667/nagiscript_lang.git ngs_driver"
  );
  process.exit(1);
}

try {
  execFileSync(bin, process.argv.slice(2), { stdio: "inherit" });
} catch (e) {
  process.exit(e.status || 1);
}
