const name = "slide_fixture_role";
const domain = "127.0.0.1";
type Role = "creator" | "viewer";
export type CookieFormat = "json" | "netscape";
export const cookieFormat = (value: string | null): value is CookieFormat => value === "json" || value === "netscape";
const validRole = (value: unknown): value is Role => value === "creator" || value === "viewer";

function fields(value: Role) {
  return { name, value, domain, path: "/", expires: 0, httpOnly: true, secure: false, hostOnly: true, sameSite: "Strict" };
}

export function exportFixtureCookie(role: Role, format: CookieFormat) {
  return format === "json" ? JSON.stringify([fields(role)], null, 2) + "\n"
    : `# Netscape HTTP Cookie File\n# Slide synthetic QA: import reconstructs SameSite=Strict.\n#HttpOnly_${domain}\tFALSE\t/\tFALSE\t0\t${name}\t${role}\n`;
}

export function importFixtureCookie(data: string, format: CookieFormat): Role {
  if (data.length > 4096) throw new Error("Use a synthetic cookie file of at most 4 KiB.");
  if (format === "json") {
    let parsed: unknown;
    try { parsed = JSON.parse(data); } catch { throw new Error("Invalid JSON."); }
    if (!Array.isArray(parsed) || parsed.length !== 1) throw new Error("Import exactly one synthetic cookie.");
    const row = parsed[0];
    if (!row || typeof row !== "object" || !validRole(row.value)) throw new Error("Only Creator or Viewer fixture roles can be imported.");
    const expected = Object.entries(fields(row.value));
    if (Object.keys(row).length !== expected.length || expected.some(([key, value]) => row[key] !== value))
      throw new Error("Only the fixture host-only, HttpOnly, SameSite=Strict, non-Secure session cookie is supported. Expiry and partitioned cookies are not accepted.");
    return row.value;
  }
  const rows = data.split(/\r?\n/).filter(line => line.trim() && (!line.startsWith("#") || line.startsWith("#HttpOnly_")));
  if (rows.length !== 1) throw new Error("Import exactly one Netscape fixture cookie.");
  const columns = rows[0].split("\t");
  if (columns.length !== 7 || columns[0] !== `#HttpOnly_${domain}` || columns[1] !== "FALSE" ||
      columns[2] !== "/" || columns[3] !== "FALSE" || columns[4] !== "0" || columns[5] !== name || !validRole(columns[6])) {
    throw new Error("Only the fixture's host-only, HttpOnly, non-Secure Netscape session row is supported; import applies SameSite=Strict.");
  }
  return columns[6];
}

export const qaControls = String.raw`
<section><h2>Synthetic storage exchange</h2>
<p>Only the fixture role cookie can be exchanged. It becomes a session-only cookie. Netscape cannot represent SameSite; import reapplies Strict. No other origins or browser credential data are read.</p>
<form action="/qa/cookie" method="post" enctype="multipart/form-data">
  <label>Format <select name="format"><option value="json">JSON</option><option value="netscape">Netscape</option></select></label>
  <label>Choose synthetic cookie file <input type="file" name="cookie" required></label>
  <button>Import into this fixture origin</button>
</form>
<a download href="/qa/cookie?format=json">Download synthetic JSON</a> &middot;
<a download href="/qa/cookie?format=netscape">Download synthetic Netscape</a>
</section>
<section><h2>Fixture-only regional and device simulation</h2>
<p>These examples change only this panel. They do not change browser viewport, language, time zone or geolocation APIs.</p>
<label>Sample preset <select id="preset"><option value="us">US &middot; desktop</option><option value="jp">Japan &middot; mobile</option></select></label>
<p><output id="environment"></output></p>
</section>
<section><h2>Deterministic text input</h2>
<label>Practice text <textarea id="practice" rows="2"></textarea></label>
<button id="fill" type="button">Fill synthetic sample</button><output id="count">0 characters</output>
</section>
<section><h2>Synthetic video source</h2>
<p>Generated color bars. This demonstration does not access or replace a camera.</p>
<video id="sample-video" width="240" height="120" muted playsinline controls></video>
<button id="play-sample" type="button">Play test video</button>
</section>
<script>
const preset = document.querySelector("#preset");
function renderPreset() {
  const jp = preset.value === "jp";
  const time = new Intl.DateTimeFormat(jp ? "ja-JP" : "en-US", { timeZone: jp ? "Asia/Tokyo" : "America/New_York", dateStyle: "medium", timeStyle: "short" }).format(new Date("2025-01-01T12:00:00Z"));
  document.querySelector("#environment").textContent = (jp ? "390×844 · ja-JP · Asia/Tokyo · 35.6762, 139.6503" : "1280×800 · en-US · America/New_York · 40.7128, −74.0060") + " · " + time;
}
preset.onchange = renderPreset; renderPreset();
const practice = document.querySelector("#practice");
practice.oninput = () => document.querySelector("#count").textContent = [...practice.value].length + " characters";
document.querySelector("#fill").onclick = () => { practice.value = "Slide QA — 雪 — 123\u000Asecond line"; practice.oninput(); };
const video = document.querySelector("#sample-video");
document.querySelector("#play-sample").onclick = async (event) => {
  const button = event.currentTarget;
  const canvas = document.createElement("canvas");
  canvas.width = 240; canvas.height = 120;
  if (!canvas.captureStream) { button.textContent = "Test video unsupported"; button.disabled = true; return; }
  const ctx = canvas.getContext("2d");
  const paint = () => {
    ["#ef4444", "#eab308", "#22c55e", "#2563eb"].forEach((color, i) => { ctx.fillStyle = color; ctx.fillRect(i * 60, 0, 60, 120); });
    ctx.fillStyle = "black"; ctx.fillRect(0, 82, 240, 38);
    ctx.fillStyle = "white"; ctx.font = "14px sans-serif"; ctx.fillText("SLIDE QA • SYNTHETIC", 20, 105);
  };
  paint();
  const stream = canvas.captureStream(4);
  const tick = setInterval(paint, 250);
  const stop = () => { clearInterval(tick); stream.getTracks().forEach(track => track.stop()); };
  addEventListener("pagehide", stop, { once: true });
  video.srcObject = stream;
  try { await video.play(); button.disabled = true; } catch { stop(); button.textContent = "Video playback unavailable"; }
};
</script>`;
