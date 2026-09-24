const $ = (id) => document.getElementById(id);
let tab = null;

function say(text, tone = "") {
  const m = $("message");
  m.textContent = text;
  m.className = "message " + tone;
}

function setState(on) {
  $("state").classList.toggle("on", on);
  $("state-text").textContent = on ? "Verbunden" : "Nicht gestartet";
  if (!on) say("omnidl startet beim Laden von selbst.");
}

async function init() {
  [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const ok = omnidlLoadable(tab?.url);
  $("title").textContent = tab?.title || "Unbenannte Seite";
  $("host").textContent = ok ? new URL(tab.url).host.replace(/^www\./, "") : "Diese Seite lässt sich nicht laden.";
  $("video").disabled = $("audio").disabled = !ok;
  if (ok) $("video").focus();
  setState(Boolean(await omnidlPing()));
}

async function load(mode) {
  $("video").disabled = $("audio").disabled = true;
  try {
    const result = await omnidlDeliver([tab.url], mode, tab.id);
    say(result === "sent" ? "In omnidl eingereiht." : "omnidl wird gestartet …", "ok");
    setTimeout(() => window.close(), result === "sent" ? 900 : 1600);
  } catch (e) {
    say(String(e.message || e), "error");
    $("video").disabled = $("audio").disabled = false;
  }
}

$("video").addEventListener("click", () => load("video"));
$("audio").addEventListener("click", () => load("audio"));
init();
