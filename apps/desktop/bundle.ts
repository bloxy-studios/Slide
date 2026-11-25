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
const fingerprintBuilds = {
  "aarch64-apple-darwin": ["148.0.7778.215", "macos.dmg", "b72f091e2e1a7583eed389c4b8e3534ed355e568af8c8bbf8fc30a25e23ca679"],
  "x86_64-apple-darwin": null,
  "x86_64-pc-windows-msvc": ["150.0.7871.186", "windows_x64.zip", "4d549c326e51ebbabf562fd365eb5380d9d4a81200da2c60f075c688d9a77e03"],
} as const;
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

async function download(path: string, url: string, checksum: string) {
  if (!await Bun.file(path).exists()) {
    const pending = `${path}.download`;
    try {
      await run(["curl", "--fail", "--location", "--retry", "2", "--max-time", "300", "--output", pending, url]);
      if (await digest(pending) !== checksum) throw new Error("Downloaded Chromium checksum mismatch.");
      await rename(pending, path);
    } finally {
      await rm(pending, { force: true });
    }
  } else if (await digest(path) !== checksum) {
    throw new Error(`Chromium checksum mismatch. Remove ${path} and rebuild.`);
  }
}

async function extractMac(image: string, app: string) {
  const mount = await mkdtemp(join(tauri, "target", "ungoogled-mount-"));
  try {
    await run(["/usr/bin/hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, image]);
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
}

async function extractWindows(zip: string, directory: string) {
  await mkdir(directory);
  await run(["tar.exe", "-xf", zip, "-C", directory, "--strip-components", "1"]);
}

await mkdir(join(tauri, "target"), { recursive: true });
await download(cache, `https://github.com/ungoogled-software/ungoogled-chromium-${platform}/releases/download/${release}/${file}`, sha256);
await rm(unpack, { recursive: true, force: true });
await mkdir(unpack);
const resources: Record<string, string> = { ...licenses };
const macFiles: Record<string, string> = {};
if (process.platform === "darwin") {
  await extractMac(cache, join(unpack, "Chromium.app"));
  macFiles["Frameworks/Chromium.app"] = `${stage}/Chromium.app`;
} else {
  await extractWindows(cache, join(unpack, "chrome-win"));
  resources[`${stage}/chrome-win/`] = "chromium/";
}
const fingerprint = fingerprintBuilds[target];
if (fingerprint) {
  const [version, asset, checksum] = fingerprint;
  const name = `ungoogled-chromium_${version}-1.1_${asset}`;
  const archive = join(tauri, "target", name);
  await download(archive, `https://github.com/adryfish/fingerprint-chromium/releases/download/${version}/${name}`, checksum);
  resources["../../../docs/fingerprint-chromium-license.txt"] = "fingerprint-chromium/LICENSE.txt";
  if (process.platform === "darwin") {
    await extractMac(archive, join(unpack, "Fingerprint Chromium.app"));
    macFiles["Frameworks/Fingerprint Chromium.app"] = `${stage}/Fingerprint Chromium.app`;
  } else {
    await extractWindows(archive, join(unpack, "fingerprint-win"));
    resources[`${stage}/fingerprint-win/`] = "fingerprint-chromium/";
  }
}
const bundle = process.platform === "darwin" ?
  { resources, macOS: { signingIdentity: "-", hardenedRuntime: false, files: macFiles } } : { resources };
const configPath = join(tauri, `${stage}.json`);
await writeFile(configPath, JSON.stringify({ bundle }));
await run(["bun", "run", "tauri", "build", "--config", configPath, ...args], process.platform === "darwin");
