import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { mkdir, rename, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

const builds = {
  "aarch64-apple-darwin": ["mac-arm", "Mac_Arm", "1753271", "chrome-mac.zip", "165574b1c45bb0b1c4fd4657e9e3889e41f69e4f72eb9cb01da8f887882d8b09"],
  "x86_64-apple-darwin": ["mac-intel", "Mac", "1753296", "chrome-mac.zip", "13f58ab268c3bbd84cce9f22f2d65c110c754242ebe8de559fa2e83a505f069b"],
  "x86_64-pc-windows-msvc": ["win-x64", "Win_x64", "1753232", "chrome-win.zip", "cb2d226d4752d69fa0184c1bb3cf98de811af4f07fa9235f3ae85d9c20b4a0b3"],
} as const;
const args = Bun.argv.slice(2);
const targetFlag = args.indexOf("--target");
const target = (targetFlag >= 0 ? args[targetFlag + 1] : process.platform === "win32" ?
  "x86_64-pc-windows-msvc" : process.arch === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin") as keyof typeof builds;
if (!Object.hasOwn(builds, target) || (process.platform === "win32" ? !target.endsWith("-msvc") :
  process.platform !== "darwin" || !target.endsWith("-darwin"))) {
  throw new Error("Bundle on macOS or Windows with one of the supported native Chromium targets.");
}
const [id, platform, revision, file, sha256] = builds[target];
const tauri = join(import.meta.dir, "src-tauri");
const stage = `target/chromium-${id}`;
const cache = join(tauri, `${stage}.zip`);
const unpack = join(tauri, stage);
const license = "../../../docs/chromium-license.txt";

async function run(...command: string[]) {
  const proc = Bun.spawn(command, { cwd: import.meta.dir, stdout: "inherit", stderr: "inherit", stdin: "inherit" });
  if (await proc.exited !== 0) throw new Error(`${command[0]} failed (exit ${proc.exitCode}).`);
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
    const url = `https://storage.googleapis.com/chromium-browser-snapshots/${platform}/${revision}/${file}`;
    await run("curl", "--fail", "--location", "--retry", "2", "--max-time", "300", "--output", pending, url);
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
  resources: { [license]: "chromium/LICENSE.txt" },
  macOS: {
    signingIdentity: "-",
    hardenedRuntime: false,
    files: { "Frameworks/Chromium.app": `${stage}/chrome-mac/Chromium.app` },
  },
} : {
  resources: { [license]: "chromium/LICENSE.txt", [`${stage}/chrome-win/`]: "chromium/" },
};
if (process.platform === "darwin") {
  await run("/usr/bin/ditto", "-x", "-k", cache, unpack);
  const app = join(unpack, "chrome-mac", "Chromium.app");
  await run("/usr/bin/codesign", "--force", "--deep", "--sign", "-", app);
  await run("/usr/bin/codesign", "--verify", "--deep", "--strict", app);
} else {
  await run("tar.exe", "-xf", cache, "-C", unpack,
    "--exclude=chrome-win/interactive_ui_tests.exe", "--exclude=chrome-win/setup.exe");
}
const configPath = join(tauri, `${stage}.json`);
await writeFile(configPath, JSON.stringify({ bundle }));
await run("bun", "run", "tauri", "build", "--config", configPath, ...args);
