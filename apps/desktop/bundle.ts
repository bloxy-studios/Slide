import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { mkdir, mkdtemp, rename, rm, rmdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

const builds = {
  "aarch64-apple-darwin": ["mac-arm", "macos", "157.0.8103.78-1.1", "arm64-macos.dmg", "bdfe0ee51e47b7ff49c94778c6d9adb08a626dfbd6089570bbbb455410f1a41a"],
  "x86_64-apple-darwin": ["mac-intel", "macos", "157.0.8103.78-1.1", "x86_64-macos.dmg", "8a7306ac6218f8b0f3c38b570922115b9545749ae4c0b2c29bbd1ebb5c00459f"],
  "x86_64-pc-windows-msvc": ["win-x64", "windows", "158.0.8127.36-1.1", "windows_x64.zip", "bc78b9590881f8ca99aca29288d1b347ffdd689ca8a57384bb0bc113ef687b6b"],
} as const;
const args = Bun.argv.slice(2);
const targetFlag = args.indexOf("--target");
const target = (targetFlag >= 0 ? args[targetFlag + 1] : process.platform === "win32" ?
  "x86_64-pc-windows-msvc" : process.arch === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin") as keyof typeof builds;
if (!Object.hasOwn(builds, target) || (process.platform === "win32" ? !target.endsWith("-msvc") :
  process.platform !== "darwin" || !target.endsWith("-darwin"))) {
  throw new Error("Bundle on macOS or Windows with one of the supported native Chromium targets.");
}
const [id, platform, release, asset, sha256] = builds[target];
const file = `ungoogled-chromium_${release}_${asset}`;
const tauri = join(import.meta.dir, "src-tauri");
const stage = `target/chromium-${id}`;
const cache = join(tauri, "target", file);
const unpack = join(tauri, stage);
const licenses = {
  "../../../docs/chromium-license.txt": "chromium/LICENSE.txt",
  "../../../docs/ungoogled-chromium-license.txt": "chromium/LICENSE-ungoogled.txt",
};

async function run(command: string[], retryBusyDmg = false) {
  for (let attempt = 1; ; attempt++) {
    const proc = Bun.spawn(command, {
      cwd: import.meta.dir, stdout: "inherit", stderr: retryBusyDmg ? "pipe" : "inherit", stdin: "inherit",
    });
    const stderr = proc.stderr ? await new Response(proc.stderr).text() : "";
    if (stderr) process.stderr.write(stderr);
    const exit = await proc.exited;
    if (exit === 0) return;
    if (attempt !== 1 || !stderr.includes("hdiutil: create failed - Resource busy")) {
      throw new Error(`${command[0]} failed (exit ${exit}).`);
    }
    console.warn("macOS disk image creation reported a busy resource; retrying once.");
  }
}

async function digest(path: string) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

await mkdir(join(tauri, "target"), { recursive: true });
if (!await Bun.file(cache).exists()) {
  const pending = `${cache}.download`;
  try {
    const url = `https://github.com/ungoogled-software/ungoogled-chromium-${platform}/releases/download/${release}/${file}`;
    await run(["curl", "--fail", "--location", "--retry", "2", "--max-time", "300", "--output", pending, url]);
    if (await digest(pending) !== sha256) throw new Error("Downloaded Chromium checksum mismatch.");
    await rename(pending, cache);
  } finally {
    await rm(pending, { force: true });
  }
} else if (await digest(cache) !== sha256) {
  throw new Error(`Chromium checksum mismatch. Remove ${cache} and rebuild.`);
}

await rm(unpack, { recursive: true, force: true });
await mkdir(unpack);
const bundle = process.platform === "darwin" ? {
  resources: licenses,
  macOS: {
    signingIdentity: "-",
    hardenedRuntime: false,
    files: { "Frameworks/Chromium.app": `${stage}/Chromium.app` },
  },
} : {
  resources: { ...licenses, [`${stage}/chrome-win/`]: "chromium/" },
};
if (process.platform === "darwin") {
  const mount = await mkdtemp(join(tauri, "target", "ungoogled-mount-"));
  const app = join(unpack, "Chromium.app");
  try {
    await run(["/usr/bin/hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, cache]);
    try {
      await run(["/usr/bin/ditto", "--norsrc", join(mount, "Chromium.app"), app]);
    } finally {
      await run(["/usr/bin/hdiutil", "detach", mount]);
    }
  } finally {
    await rmdir(mount);
  }
  await run(["/usr/bin/codesign", "--force", "--deep", "--sign", "-", app]);
  await run(["/usr/bin/codesign", "--verify", "--deep", "--strict", app]);
} else {
  const windows = join(unpack, "chrome-win");
  await mkdir(windows);
  await run(["tar.exe", "-xf", cache, "-C", windows, "--strip-components", "1"]);
}
const configPath = join(tauri, `${stage}.json`);
await writeFile(configPath, JSON.stringify({ bundle }));
await run(["bun", "run", "tauri", "build", "--config", configPath, ...args], process.platform === "darwin");
