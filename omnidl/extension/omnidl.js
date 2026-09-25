// Talks to the omnidl app on this computer. Shared by popup and background.
const OMNIDL_API = "http://127.0.0.1:47813/v1";
const OMNIDL_HEADERS = { "X-Omnidl-Client": "extension" };

/** App info, or null when omnidl is not running. */
async function omnidlPing() {
  try {
    const r = await fetch(`${OMNIDL_API}/ping`, { headers: OMNIDL_HEADERS });
    return r.ok ? await r.json() : null;
  } catch {
    return null;
  }
}

async function omnidlSend(urls, mode) {
  const r = await fetch(`${OMNIDL_API}/add`, {
    method: "POST",
    headers: { ...OMNIDL_HEADERS, "Content-Type": "application/json" },
    body: JSON.stringify({ urls, mode }),
  });
  if (!r.ok) throw new Error(`omnidl antwortet mit ${r.status}`);
  return r.json();
}

/** omnidl://add?url=…  starts the app with the links when it is not running. */
function omnidlLink(urls, mode) {
  const parts = urls.map((u) => "url=" + encodeURIComponent(u));
  if (mode) parts.push("mode=" + mode);
  return "omnidl://add?" + parts.join("&");
}

function omnidlLoadable(url) {
  return /^https?:\/\//i.test(url || "");
}

/**
 * Hands the links over. Returns "sent" when the running app took them,
 * "launched" when the browser was asked to start omnidl with them.
 */
async function omnidlDeliver(urls, mode, tabId) {
  try {
    await omnidlSend(urls, mode);
    return "sent";
  } catch {
    await chrome.tabs.update(tabId, { url: omnidlLink(urls, mode) });
    return "launched";
  }
}
