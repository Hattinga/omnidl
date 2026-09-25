// Chrome loads one service worker file; Firefox loads omnidl.js itself (manifest).
if (typeof importScripts === "function" && typeof omnidlDeliver === "undefined") {
  importScripts("omnidl.js");
}

const MENUS = [
  { id: "link:video", title: "Link als Video laden", contexts: ["link"] },
  { id: "link:audio", title: "Link als Audio laden", contexts: ["link"] },
  { id: "page:video", title: "Seite als Video laden", contexts: ["page", "video", "audio", "image"] },
  { id: "page:audio", title: "Seite als Audio laden", contexts: ["page", "video", "audio", "image"] },
];

chrome.runtime.onInstalled.addListener(async () => {
  await chrome.contextMenus.removeAll();
  for (const m of MENUS) chrome.contextMenus.create(m);
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const [what, mode] = String(info.menuItemId).split(":");
  const url = what === "link" ? info.linkUrl : info.pageUrl || tab?.url;
  if (!omnidlLoadable(url) || !tab?.id) return;
  const result = await omnidlDeliver([url], mode, tab.id);
  badge(tab.id, result === "sent" ? "✓" : "…");
});

function badge(tabId, text) {
  chrome.action.setBadgeBackgroundColor({ color: "#34C759" });
  chrome.action.setBadgeText({ text, tabId });
  setTimeout(() => chrome.action.setBadgeText({ text: "", tabId }), 2500);
}
