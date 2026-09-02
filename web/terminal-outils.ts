// Outils du terminal : zoom de police, recherche, menu clic droit.

import { invoke } from "@tauri-apps/api/core";
import { $, FONT_MAX, FONT_MIN, state } from "./etat";
import { collerDansTerminal } from "./dialogues";

// ---------- Zoom de police, recherche, menu clic droit ----------

/** Applique une taille de police à tous les terminaux et la borne. */
export function setFontSize(px: number) {
  state.terminalFontSize = Math.max(FONT_MIN, Math.min(FONT_MAX, px));
  for (const s of state.sessions.values()) {
    s.term.options.fontSize = state.terminalFontSize;
    s.fit.fit();
    invoke("pty_resize", { id: s.id, cols: s.term.cols, rows: s.term.rows }).catch(() => {});
  }
}

/** Ouvre la barre de recherche du terminal actif. */
export function openTermSearch(id: number) {
  if (state.active !== id) return;
  const bar = $("term-search");
  bar.classList.add("open");
  const input = $("term-search-input") as HTMLInputElement;
  input.value = "";
  input.focus();
}

function closeTermSearch() {
  $("term-search").classList.remove("open");
  const s = state.active !== null ? state.sessions.get(state.active) : undefined;
  s?.search.clearDecorations();
  s?.term.focus();
}

function runTermSearch(next: boolean) {
  if (state.active === null) return;
  const s = state.sessions.get(state.active);
  if (!s) return;
  const q = ($("term-search-input") as HTMLInputElement).value;
  if (!q) return;
  const opts = { decorations: { matchOverviewRuler: "#8b7cf6", activeMatchColorOverviewRuler: "#a598f8" } };
  if (next) s.search.findNext(q, opts);
  else s.search.findPrevious(q, opts);
}

$("term-search-input").addEventListener("keydown", (e) => {
  const k = e as KeyboardEvent;
  if (k.key === "Enter") { k.preventDefault(); runTermSearch(!k.shiftKey); }
  else if (k.key === "Escape") { k.preventDefault(); closeTermSearch(); }
});
$("term-search-next").addEventListener("click", () => runTermSearch(true));
$("term-search-prev").addEventListener("click", () => runTermSearch(false));
$("term-search-close").addEventListener("click", closeTermSearch);

// Menu contextuel du terminal : copier / coller / rechercher / tout sélectionner.
const ctxMenu = () => $("term-context");
function hideContext() { ctxMenu().classList.remove("open"); }

$("terminal").addEventListener("contextmenu", (e) => {
  e.preventDefault();
  if (state.active === null) return;
  const m = ctxMenu();
  const s = state.sessions.get(state.active);
  const hasSel = !!s?.term.getSelection();
  // Griser « copier » sans sélection.
  (m.querySelector('[data-act="copy"]') as HTMLElement).classList.toggle("disabled", !hasSel);
  m.style.left = `${(e as MouseEvent).clientX}px`;
  m.style.top = `${(e as MouseEvent).clientY}px`;
  m.classList.add("open");
});
window.addEventListener("click", hideContext);
window.addEventListener("blur", hideContext);
ctxMenu().addEventListener("click", (e) => {
  const act = (e.target as HTMLElement).closest("[data-act]")?.getAttribute("data-act");
  const s = state.active !== null ? state.sessions.get(state.active) : undefined;
  if (!s) return;
  if (act === "copy") {
    const sel = s.term.getSelection();
    if (sel) navigator.clipboard.writeText(sel).catch(() => {});
  } else if (act === "paste") {
    navigator.clipboard.readText().then(
      (t) => void collerDansTerminal(s.term, t),
      () => {},
    );
  } else if (act === "search") {
    openTermSearch(s.id);
  } else if (act === "selectall") {
    s.term.selectAll();
  } else if (act === "clear") {
    s.term.clear();
  }
  hideContext();
});
