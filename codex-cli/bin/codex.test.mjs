import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import assert from "node:assert/strict";

const TARGET_BY_PLATFORM_ARCH = {
  "linux-x64": "x86_64-unknown-linux-musl",
  "linux-arm64": "aarch64-unknown-linux-musl",
  "darwin-x64": "x86_64-apple-darwin",
  "darwin-arm64": "aarch64-apple-darwin",
  "win32-x64": "x86_64-pc-windows-msvc",
  "win32-arm64": "aarch64-pc-windows-msvc",
};

function currentTargetTriple() {
  const target = TARGET_BY_PLATFORM_ARCH[`${process.platform}-${process.arch}`];
  assert.ok(target, `unsupported test platform ${process.platform}-${process.arch}`);
  return target;
}

function writeFakeNativeBinary(root) {
  const binaryName = process.platform === "win32" ? "codex.exe" : "codex";
  const binaryPath = path.join(root, "vendor", currentTargetTriple(), "bin", binaryName);
  mkdirSync(path.dirname(binaryPath), { recursive: true });
  writeFileSync(
    binaryPath,
    [
      "#!/usr/bin/env node",
      "const fs = await import('node:fs');",
      "console.log(JSON.stringify({",
      "  CODEX_HOME: process.env.CODEX_HOME,",
      "  IAPPCODEX_HOME: process.env.IAPPCODEX_HOME,",
      "  argv: process.argv.slice(2),",
      "}));",
      "",
    ].join("\n"),
  );
  chmodSync(binaryPath, 0o755);
}

function stageShim() {
  const root = mkdtempSync(path.join(tmpdir(), "iappcodex-shim-"));
  const binDir = path.join(root, "bin");
  mkdirSync(binDir, { recursive: true });
  copyFileSync(new URL("./codex.js", import.meta.url), path.join(binDir, "codex.js"));
  writeFakeNativeBinary(root);
  return root;
}

test("iappcodex shim defaults CODEX_HOME to ~/.hasna/internalapps/codex", () => {
  const root = stageShim();
  const home = path.join(root, "home");
  const regularCodexHome = path.join(home, ".codex");
  const iappCodexHome = path.join(home, ".hasna", "internalapps", "codex");
  mkdirSync(home);
  mkdirSync(regularCodexHome);
  writeFileSync(path.join(regularCodexHome, "auth.json"), "{}");

  const output = execFileSync(process.execPath, [path.join(root, "bin", "codex.js"), "login", "status"], {
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      CODEX_HOME: regularCodexHome,
      IAPPCODEX_HOME: "",
    },
    encoding: "utf8",
  });

  assert.deepEqual(JSON.parse(output), {
    CODEX_HOME: iappCodexHome,
    IAPPCODEX_HOME: "",
    argv: ["login", "status"],
  });
  assert.equal(existsSync(iappCodexHome), true);
  assert.equal(existsSync(path.join(iappCodexHome, "auth.json")), false);
  if (process.platform !== "win32") {
    assert.equal(statSync(iappCodexHome).mode & 0o777, 0o700);
  }
});

test("iappcodex shim lets IAPPCODEX_HOME override the default home", () => {
  const root = stageShim();
  const home = path.join(root, "home");
  const iappHome = path.join(root, "custom-iapp-home");
  mkdirSync(home);

  const output = execFileSync(process.execPath, [path.join(root, "bin", "codex.js"), "exec"], {
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      CODEX_HOME: "",
      IAPPCODEX_HOME: iappHome,
    },
    encoding: "utf8",
  });

  assert.deepEqual(JSON.parse(output), {
    CODEX_HOME: iappHome,
    IAPPCODEX_HOME: iappHome,
    argv: ["exec"],
  });
});

test("iappcodex shim ignores inherited CODEX_HOME and uses IAPPCODEX_HOME", () => {
  const root = stageShim();
  const home = path.join(root, "home");
  const codexHome = path.join(home, ".codex");
  const iappHome = path.join(root, "custom-iapp-home");
  mkdirSync(home);
  mkdirSync(codexHome);
  writeFileSync(path.join(codexHome, "auth.json"), "{}");

  const output = execFileSync(process.execPath, [path.join(root, "bin", "codex.js"), "--version"], {
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      CODEX_HOME: codexHome,
      IAPPCODEX_HOME: iappHome,
    },
    encoding: "utf8",
  });

  assert.deepEqual(JSON.parse(output), {
    CODEX_HOME: iappHome,
    IAPPCODEX_HOME: iappHome,
    argv: ["--version"],
  });
  assert.equal(existsSync(path.join(iappHome, "auth.json")), false);
});
