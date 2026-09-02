// Thème de l'interface : système (défaut), clair ou sombre.

import { ic } from "./icons";
import { allTags } from "./filters";
import { $, THEME_DARK, THEME_LIGHT, state } from "./etat";
import { renderHosts } from "./main";
import { t } from "./i18n";

// --- Thème de l'interface : système (défaut), clair, ou sombre ---

type ThemePref = "system" | "light" | "dark";
const THEME_KEY = "avash.theme";
const systemDark = window.matchMedia("(prefers-color-scheme: dark)");

function readThemePref(): ThemePref {
  try {
    const v = localStorage.getItem(THEME_KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch { /* stockage indispo */ }
  return "system";
}
let themePref: ThemePref = readThemePref();

/** Sombre effectif, une fois la préférence système résolue. */
function isDark(): boolean {
  return themePref === "dark" || (themePref === "system" && systemDark.matches);
}

export function terminalTheme() {
  return isDark() ? THEME_DARK : THEME_LIGHT;
}

/** Applique la préférence : attribut racine, terminaux ouverts, bouton. */
export function applyTheme() {
  const root = document.documentElement;
  if (themePref === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", themePref);
  const th = terminalTheme();
  for (const s of state.sessions.values()) s.term.options.theme = th;
  const btn = $("theme-toggle");
  const icon = themePref === "system" ? "monitor" : themePref === "light" ? "sun" : "moon";
  btn.innerHTML = ic(icon);
  btn.title = t("theme-titre", { mode: t(themePref === "system" ? "theme-systeme" : themePref === "light" ? "theme-clair" : "theme-sombre") });
}

export function cycleTheme() {
  themePref = themePref === "system" ? "light" : themePref === "light" ? "dark" : "system";
  try { localStorage.setItem(THEME_KEY, themePref); } catch { /* stockage indispo */ }
  applyTheme();
}

// Le système change (nuit/jour) : ne recolorer que si on le suit.
systemDark.addEventListener("change", () => { if (themePref === "system") applyTheme(); });

/**
 * Nerd Font en tete : l'invite d'un shell moderne (fish, starship, powerlevel)
 * utilise des glyphes que les polices classiques ne contiennent pas et
 * remplacent par des carres vides.
 */
export const FONT_STACK =
  '"Avash Mono", "MesloLGS Nerd Font Mono", "Hack", "Consolas", ' +
  '"DejaVu Sans Mono", ui-monospace, monospace';


/**
 * Attend que la police embarquee soit chargee.
 *
 * xterm.js mesure la largeur d'un caractere a l'initialisation. Si la police
 * arrive apres, il garde les metriques de la police de repli : colonnes
 * decalees, curseur mal place, cadres semi-graphiques disjoints. On attend
 * donc une fois, au demarrage, avant d'ouvrir le moindre terminal.
 */
let fontReady: Promise<void> | null = null;

export function ensureFontLoaded(): Promise<void> {
  if (!fontReady) {
    fontReady = (async () => {
      try {
        await Promise.all([
          document.fonts.load('400 14px "Avash Mono"'),
          document.fonts.load('600 14px "Avash Mono"'),
        ]);
        await document.fonts.ready;
      } catch {
        // Police indisponible : on continue avec la pile de repli plutot
        // que de bloquer l'ouverture d'un terminal.
      }
    })();
  }
  return fontReady;
}

/** Etat agrege des sessions d'un hote, pour la pastille de la liste. */
export function hostSessionState(alias: string): "" | "live" | "connecting" {
  let st: "" | "live" | "connecting" = "";
  for (const s of state.sessions.values()) {
    if (s.alias !== alias || s.closed) continue;
    const dot = s.tab.querySelector(".state");
    if (dot?.classList.contains("live")) return "live";
    if (dot?.classList.contains("connecting")) st = "connecting";
  }
  return st;
}

/** Barre de filtres par tag, sous l'en-tete « Hôtes ». */
export function renderTagBar() {
  const bar = $("tag-bar");
  const tags = allTags(state.hosts);
  if (tags.length === 0) { bar.hidden = true; return; }
  bar.hidden = false;
  bar.innerHTML = "";
  for (const tag of tags) {
    // La barre de filtres est devenue le seul endroit où consulter les tags —
    // les pastilles ont quitté les lignes d'hôte. Elle ne peut donc plus être
    // le seul contrôle hors d'atteinte au clavier.
    const c = document.createElement("button");
    c.type = "button";
    c.className = "tag-pill" + (tag === state.tagFilter ? " on" : "");
    c.textContent = tag;
    c.setAttribute("aria-pressed", String(tag === state.tagFilter));
    c.title = t("theme-filtrer-par", { tag });
    c.addEventListener("click", () => {
      state.tagFilter = state.tagFilter === tag ? null : tag;
      renderHosts();
    });
    bar.appendChild(c);
  }
  if (state.tagFilter) {
    const clear = document.createElement("button");
    clear.type = "button";
    clear.className = "tag-pill clear";
    clear.textContent = "✕";
    clear.title = t("theme-effacer-filtre");
    clear.addEventListener("click", () => { state.tagFilter = null; renderHosts(); });
    bar.appendChild(clear);
  }
}
