if (["4310", "4311"].includes(location.port)) {
  const badge = document.createElement("aside");
  badge.textContent = `Slide fixture extension · v${chrome.runtime.getManifest().version}`;
  badge.style.cssText = "position:fixed;right:16px;bottom:16px;padding:8px 12px;border:1px solid #9ce7bc;border-radius:8px;background:#102419;color:#e3fff1;font:14px system-ui;z-index:10000";
  document.body.append(badge);
}
