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

function writeFakeNativeBinary(root, binaryStem = "codewith") {
  const binaryName = process.platform === "win32" ? `${binaryStem}.exe` : binaryStem;
  const binaryPath = path.join(root, "vendor", currentTargetTriple(), "bin", binaryName);
  mkdirSync(path.dirname(binaryPath), { recursive: true });
  writeFileSync(
    binaryPath,
    [
      "#!/usr/bin/env node",
      "const fs = await import('node:fs');",
      "console.log(JSON.stringify({",
      "  CODEX_HOME: process.env.CODEX_HOME,",
      "  CODEWITH_HOME: process.env.CODEWITH_HOME,",
      "  argv: process.argv.slice(2),",
      "}));",
      "",
    ].join("\n"),
  );
  chmodSync(binaryPath, 0o755);
}

function stageShim(binaryStem = "codewith") {
  const root = mkdtempSync(path.join(tmpdir(), "codewith-shim-"));
  const binDir = path.join(root, "bin");
  mkdirSync(binDir, { recursive: true });
  copyFileSync(new URL("./codex.js", import.meta.url), path.join(binDir, "codex.js"));
  writeFakeNativeBinary(root, binaryStem);
  return root;
}

test("codewith shim defaults CODEX_HOME to ~/.codewith", () => {
  const root = stageShim();
  const home = path.join(root, "home");
  const regularCodexHome = path.join(home, ".codex");
  const codewithHome = path.join(home, ".codewith");
  mkdirSync(home);
  mkdirSync(regularCodexHome);
  writeFileSync(path.join(regularCodexHome, "auth.json"), "{}");

  const output = execFileSync(process.execPath, [path.join(root, "bin", "codex.js"), "login", "status"], {
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      CODEX_HOME: "",
      CODEWITH_HOME: "",
    },
    encoding: "utf8",
  });

  assert.deepEqual(JSON.parse(output), {
    CODEX_HOME: codewithHome,
    CODEWITH_HOME: codewithHome,
    argv: ["login", "status"],
  });
  assert.equal(existsSync(codewithHome), true);
  assert.equal(existsSync(path.join(codewithHome, "auth.json")), false);
  if (process.platform !== "win32") {
    assert.equal(statSync(codewithHome).mode & 0o777, 0o700);
  }
});

test("codewith shim honors CODEX_HOME as a compatibility fallback", () => {
  const root = stageShim();
  const home = path.join(root, "home");
  const legacyConfiguredHome = path.join(root, "legacy-configured-home");
  mkdirSync(home);

  const output = execFileSync(process.execPath, [path.join(root, "bin", "codex.js"), "exec"], {
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      CODEX_HOME: legacyConfiguredHome,
      CODEWITH_HOME: "",
    },
    encoding: "utf8",
  });

  assert.deepEqual(JSON.parse(output), {
    CODEX_HOME: legacyConfiguredHome,
    CODEWITH_HOME: legacyConfiguredHome,
    argv: ["exec"],
  });
});

test("codewith shim lets CODEWITH_HOME override the default home", () => {
  const root = stageShim();
  const home = path.join(root, "home");
  const codewithHome = path.join(root, "custom-codewith-home");
  mkdirSync(home);

  const output = execFileSync(process.execPath, [path.join(root, "bin", "codex.js"), "exec"], {
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      CODEX_HOME: "",
      CODEWITH_HOME: codewithHome,
    },
    encoding: "utf8",
  });

  assert.deepEqual(JSON.parse(output), {
    CODEX_HOME: codewithHome,
    CODEWITH_HOME: codewithHome,
    argv: ["exec"],
  });
});

test("codewith shim lets CODEWITH_HOME override CODEX_HOME", () => {
  const root = stageShim();
  const home = path.join(root, "home");
  const codexHome = path.join(home, ".codex");
  const codewithHome = path.join(root, "custom-codewith-home");
  mkdirSync(home);
  mkdirSync(codexHome);
  writeFileSync(path.join(codexHome, "auth.json"), "{}");

  const output = execFileSync(process.execPath, [path.join(root, "bin", "codex.js"), "--version"], {
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      CODEX_HOME: codexHome,
      CODEWITH_HOME: codewithHome,
    },
    encoding: "utf8",
  });

  assert.deepEqual(JSON.parse(output), {
    CODEX_HOME: codewithHome,
    CODEWITH_HOME: codewithHome,
    argv: ["--version"],
  });
  assert.equal(existsSync(path.join(codewithHome, "auth.json")), false);
});

test("codewith shim tolerates read-only existing CODEWITH_HOME chmod", () => {
  const root = stageShim();
  const home = path.join(root, "home");
  const codewithHome = path.join(root, "readonly-codewith-home");
  const preloadPath = path.join(root, "mock-chmod-erofs.mjs");
  mkdirSync(home);
  mkdirSync(codewithHome);
  if (process.platform !== "win32") {
    chmodSync(codewithHome, 0o700);
  }
  writeFileSync(
    preloadPath,
    [
      "import fs from 'node:fs';",
      "import { syncBuiltinESMExports } from 'node:module';",
      "const realChmodSync = fs.chmodSync;",
      `const blockedPath = ${JSON.stringify(codewithHome)};`,
      "fs.chmodSync = (path, mode) => {",
      "  if (path === blockedPath) {",
      "    const err = new Error(`EROFS: read-only file system, chmod '${path}'`);",
      "    err.code = 'EROFS';",
      "    err.path = path;",
      "    err.syscall = 'chmod';",
      "    throw err;",
      "  }",
      "  return realChmodSync(path, mode);",
      "};",
      "syncBuiltinESMExports();",
      "",
    ].join("\n"),
  );

  const output = execFileSync(process.execPath, [path.join(root, "bin", "codex.js"), "login", "status"], {
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      CODEX_HOME: "",
      CODEWITH_HOME: codewithHome,
      NODE_OPTIONS: `--import=${preloadPath}`,
    },
    encoding: "utf8",
  });

  assert.deepEqual(JSON.parse(output), {
    CODEX_HOME: codewithHome,
    CODEWITH_HOME: codewithHome,
    argv: ["login", "status"],
  });
});

test("codewith shim supports legacy codex native binary packages", () => {
  const root = stageShim("codex");
  const home = path.join(root, "home");
  mkdirSync(home);

  const output = execFileSync(process.execPath, [path.join(root, "bin", "codex.js"), "--version"], {
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      CODEX_HOME: "",
      CODEWITH_HOME: "",
    },
    encoding: "utf8",
  });

  assert.deepEqual(JSON.parse(output), {
    CODEX_HOME: path.join(home, ".codewith"),
    CODEWITH_HOME: path.join(home, ".codewith"),
    argv: ["--version"],
  });
});
