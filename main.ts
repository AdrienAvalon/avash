// Avash front v0.2 — terminal interactif réel : xterm.js ↔ PTY Rust (russh)

import "@xterm/xterm/css/xterm.css";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { humanSize, filterHosts, remoteJoin, type Host } from "./filters";

type Session = {
  id: number;
  alias: string;
  term: Terminal;
  fit: FitAddon;
  tab: HTMLElement;
};

const state = {
  hosts: [] as Host[],
  filter: "",
  nextId: 1,
  active: null as number | null,
  sessions: new Map<number, Session>(),
};

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const THEME = {
  background: "#0f1117",
  foreground: "#e6e9f0",
  cursor: "#7c6cf5",
  cursorAccent: "#0f1117",
  selectionBackground: "rgba(124, 108, 245, .35)",
  black: "#0f1117", red: "#f56a6a", green: "#3fd68f", yellow: "#f5b652",
  blue: "#6c9ef5", magenta: "#b57cf5", cyan: "#6cd9f5", white: "#e6e9f0",
  brightBlack: "#5c6379", brightRed: "#ff8c8c", brightGreen: "#6ce8ae",
  brightYellow: "#ffce7a", brightBlue: "#93b8ff", brightMagenta: "#d0a0ff",
  brightCyan: "#96e8ff", brightWhite: "#ffffff",
  fontFamily: '"JetBrains Mono", "Fira Code", ui-monospace, monospace',
};

function renderHosts() {
  const list = $("host-list");
  list.innerHTML = "";
  const shown = filterHosts(state.hosts, state.filter);
  for (const h of shown) {
    const el = document.createElement("div");
    el.className = "host" + (state.sessions.has(state.active ?? -1) && state.sessions.get(state.active!)?.alias === h.alias ? " selected" : "");
    const target = `${h.user ?? "?"}@${h.hostname ?? h.alias}:${h.port ?? 22}`;
    el.innerHTML = `<span class="dot ok"></span><span class="info">
      <div class="alias"></div><div class="meta"></div></span>`;
    el.querySelector(".alias")!.textContent = h.alias;
    el.querySelector(".meta")!.textContent = target;
    el.title = "Double-clic : connexion";
    el.addEventListener("dblclick", () => openSession(h));
    list.appendChild(el);
  }
  $("host-count").textContent = `${shown.length} hôte${shown.length > 1 ? "s" : ""}`;
}

async function openSession(h: Host) {
  const id = state.nextId++;
  const term = new Terminal({
    theme: THEME,
    fontSize: 13.5,
    cursorBlink: true,
    scrollback: 5000,
    macOptionIsMeta: true,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);

  // Onglet
  const tabs = $("tabs");
  tabs.querySelector(".no-session")?.remove();
  const tab = document.createElement("div");
  tab.className = "tab active";
  tab.innerHTML = `<span class="label"></span><span class="close">✕</span>`;
  tab.querySelector(".label")!.textContent = h.alias;
  tab.addEventListener("click", () => focusSession(id));
  tab.querySelector(".close")!.addEventListener("click", (e) => {
    e.stopPropagation();
    closeSession(id);
  });
  tabs.querySelectorAll(".tab").forEach((t) => t.classList.remove("active"));
  tabs.appendChild(tab);

  // Zone terminal : un container par session, hidden si inactif
  $("terminal-empty").style.display = "none";
  const container = document.createElement("div");
  container.className = "xterm-container";
  container.style.position = "absolute";
  container.style.inset = "8px";
  $("terminal").appendChild(container);
  term.open(container);
  term.onData((data) => {
    invoke("pty_write", { id, data }).catch((e) => term.write(`\r\n⚠️ write: ${e}\r\n`));
  });
  term.onResize(({ cols, rows }) => {
    invoke("pty_resize", { id, cols, rows }).catch(() => {});
  });

  const s: Session = { id, alias: h.alias, term, fit, tab };
  state.sessions.set(id, s);
  state.active = id;
  focusTerminal(s);

  try {
    await invoke("pty_open", { id, alias: h.alias, cols: term.cols, rows: term.rows });
  } catch (e) {
    term.write(`\x1b[31m⚔️ Échec connexion : ${e}\x1b[0m\r\n`);
  }
}

function focusTerminal(s: Session) {
  s.term.focus();
  requestAnimationFrame(() => s.fit.fit());
}

function focusSession(id: number) {
  state.active = id;
  state.sessions.forEach((s, sid) => {
    const active = sid === id;
    s.tab.classList.toggle("active", active);
    (s.term.element?.parentElement as HTMLElement).style.display = active ? "block" : "none";
    if (active) focusTerminal(s);
  });
}

function closeSession(id: number) {
  const s = state.sessions.get(id);
  if (!s) return;
  invoke("pty_close", { id }).catch(() => {});
  s.term.dispose();
  s.tab.remove();
  (s.term.element?.parentElement)?.remove();
  state.sessions.delete(id);
  if (state.active === id) {
    const first = state.sessions.keys().next();
    if (first.done) {
      state.active = null;
      $("terminal-empty").style.display = "flex";
    } else {
      focusSession(first.value);
    }
  }
}

// Écoute du flux PTY côté Rust → xterm
type PtyPayload = { id: number; data: string };
listenPty();
async function listenPty() {
  try {
    await listen<PtyPayload>("pty-output", (ev) => {
      const s = state.sessions.get(ev.payload.id);
      if (s && ev.payload.id === state.active) {
        s.term.write(ev.payload.data);
      } else if (s) {
        // bufferise même si pas actif : le terminal xterm stocke
        s.term.write(ev.payload.data);
      }
    });
  } catch (e) {
    console.warn("Bridge Tauri absent (dev navigateur) :", e);
  }
}

async function loadHosts() {
  try {
    state.hosts = await invoke<Host[]>("list_hosts");
  } catch (e) {
    console.warn("Config SSH illisible :", e);
  }
  renderHosts();
}

// Search sidebar
const searchEl = $("search") as HTMLInputElement;
searchEl.addEventListener("input", () => { state.filter = searchEl.value; renderHosts(); });

// Palette
const paletteEl = $("palette") as HTMLDivElement;
const paletteInput = $("palette-input") as HTMLInputElement;
function paletteOpen() {
  paletteEl.classList.add("open");
  paletteInput.value = "";
  renderPalette();
  paletteInput.focus();
}
function paletteClose() { paletteEl.classList.remove("open"); }
function renderPalette() {
  const q = paletteInput.value.toLowerCase();
  const res = $("palette-results");
  res.innerHTML = "";
  const matches = filterHosts(state.hosts, q);
  if (matches.length === 0) {
    res.innerHTML = `<div class="empty">Aucun hôte pour « ${q.replace(/[<>&]/g, "")} »</div>`;
    return;
  }
  for (const h of matches) {
    const item = document.createElement("div");
    item.className = "item";
    item.innerHTML = `<span>😈</span><span class="name"></span><span class="sub"></span>`;
    item.querySelector(".name")!.textContent = h.alias;
    item.querySelector(".sub")!.textContent = `${h.user ?? "?"}@${h.hostname ?? h.alias}`;
    item.addEventListener("click", () => { paletteClose(); openSession(h); });
    res.appendChild(item);
  }
}
paletteInput.addEventListener("input", renderPalette);
paletteInput.addEventListener("keydown", (e) => { if (e.key === "Escape") paletteClose(); });
paletteEl.addEventListener("click", (e) => { if (e.target === paletteEl) paletteClose(); });
document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
    e.preventDefault();
    paletteOpen();
  }
});

window.addEventListener("resize", () => {
  if (state.active !== null) {
    const s = state.sessions.get(state.active);
    if (s) s.fit.fit();
  }
});

// ===== SFTP =====
type SftpEntry = { name: string; is_dir: boolean; size: number; modified: number | null };

const sftp = {
  path: "" as string,
  open: false,
};


async function sftpNavigate(path: string) {
  const id = state.active;
  if (id === null) return;
  sftp.path = path;
  $("sftp-path").textContent = path;
  $("sftp-list").innerHTML = `<div class="sftp-status">Chargement…</div>`;
  try {
    const entries = await invoke<SftpEntry[]>("sftp_list", { id, path });
    const list = $("sftp-list");
    list.innerHTML = "";
    // Dossiers d'abord, tri alpha
    entries.sort((a, b) => (b.is_dir ? 1 : 0) - (a.is_dir ? 1 : 0) || a.name.localeCompare(b.name));
    if (path !== "/") {
      const up = document.createElement("div");
      up.className = "sftp-entry dir";
      up.innerHTML = `<span>📁</span><span class="nm">..</span>`;
      up.addEventListener("click", () => {
        const parent = path.replace(/\/[^/]+\/?$/, "") || "/";
        sftpNavigate(parent);
      });
      list.appendChild(up);
    }
    for (const e of entries) {
      const el = document.createElement("div");
      el.className = "sftp-entry" + (e.is_dir ? " dir" : "");
      el.innerHTML = `<span>${e.is_dir ? "📁" : "📄"}</span><span class="nm"></span><span class="sz"></span>`;
      el.querySelector(".nm")!.textContent = e.name;
      el.querySelector(".sz")!.textContent = e.is_dir ? "" : humanSize(e.size);
      if (e.is_dir) {
        el.addEventListener("click", () =>
          sftpNavigate(remoteJoin(path, e.name))
        );
      } else {
        el.title = "Clic : télécharger";
        el.addEventListener("click", async () => {
          const remote = remoteJoin(path, e.name);
          $("sftp-status").textContent = `⬇︎ ${e.name}…`;
          try {
            const res = await invoke<string>("sftp_download", { id, remote });
            $("sftp-status").textContent = `✅ ${res}`;
          } catch (err) {
            $("sftp-status").textContent = `⚠️ ${err}`;
          }
        });
      }
      list.appendChild(el);
    }
    $("sftp-status").textContent = `${entries.length} éléments`;
  } catch (e) {
    $("sftp-list").innerHTML = "";
    $("sftp-status").textContent = `⚠️ ${e}`;
  }
}

function sftpToggle() {
  const id = state.active;
  if (id === null) return;
  sftp.open = !sftp.open;
  $("sftp-panel").classList.toggle("open", sftp.open);
  if (sftp.open) {
    // "." = cwd du serveur au login (home en général)
    sftpNavigate(sftp.path.length > 0 ? sftp.path : ".");
  }
}

// Toggle SFTP : bouton rafraîchissement
$("sftp-refresh-btn").addEventListener("click", () => sftpNavigate(sftp.path || "."));
document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "b") {
    e.preventDefault();
    sftpToggle();
  }
});

loadHosts();