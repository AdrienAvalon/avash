// État partagé du front : hôtes, sessions, bureaux RDP, réglages, thèmes du terminal et l'accès au DOM (`$`).

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { SerializeAddon } from "@xterm/addon-serialize";
import { type Host, type OsInfo } from "./filters";

// ---------- Systeme distant par hote ----------
//
// Detecte a chaque ouverture de session (evenement `host-os`), memorise
// dans localStorage pour afficher le logo des le lancement suivant, avant
// meme de se connecter.

const OS_CACHE_KEY = "avash.os.v1";
export const osByHost = new Map<string, OsInfo>();
try {
  const raw = localStorage.getItem(OS_CACHE_KEY);
  if (raw) for (const [k, v] of Object.entries(JSON.parse(raw) as Record<string, OsInfo>)) osByHost.set(k, v);
} catch { /* cache absent ou corrompu : on repart de zero */ }

export function rememberOs(label: string, os: OsInfo) {
  osByHost.set(label, os);
  try {
    localStorage.setItem(OS_CACHE_KEY, JSON.stringify(Object.fromEntries(osByHost)));
  } catch { /* stockage indisponible : le logo vivra le temps de la session */ }
}

export type Session = {
  id: number;
  alias: string;
  term: Terminal;
  fit: FitAddon;
  tab: HTMLElement;
  search: SearchAddon;
  /** Sérialise l'écran (séquences comprises) : état initial d'un enregistrement. */
  serialiser: SerializeAddon;
  /** Session terminee cote serveur : le clavier ne part plus au shell. */
  closed: boolean;
  /** Rouvre la meme cible dans ce meme onglet (Entree apres deconnexion). */
  reconnect: (() => Promise<void>) | null;
  /** Dossier distant courant du panneau SFTP, propre a chaque onglet. */
  sftpPath: string;
};

/** Bureau RDP enregistré (`~/.config/avash/rdp.yaml`). */
export type RdpHostT = { id: string; name: string; host: string; port: number; user: string; width: number; height: number; folder: string; sans_nla?: boolean; protocole?: "rdp" | "vnc" };

export const FONT_MIN = 9;
export const FONT_MAX = 28;

export const state = {
  hosts: [] as Host[],
  /** Bureaux RDP enregistrés, rechargés avec les hôtes SSH. */
  rdpHosts: [] as RdpHostT[],
  /** Taille de police partagée par tous les terminaux (Ctrl +/−). */
  terminalFontSize: 14,
  filter: "",
  nextId: 1,
  active: null as number | null,
  sessions: new Map<number, Session>(),
  /** Hote surligne par un simple clic (sans connexion). */
  pickedAlias: null as string | null,
  /** Bureau RDP surligné par un simple clic (id). */
  pickedRdp: null as string | null,
  /** Filtre par tag actif (null = tous). */
  tagFilter: null as string | null,
  /** Dossiers connus (registre + dérivés des hôtes), triés. */
  folders: [] as string[],
  /** Dernière sonde de santé, par clé de ligne (`ssh:alias`, `rdp:id`). */
  sante: new Map<string, Sante>(),
};

/** Ce qu'une sonde de santé a vu d'un hôte (voir `hosts_health`). */
export type Sante =
  | { etat: "joignable"; latence_ms: number }
  | { etat: "injoignable"; raison: string }
  | { etat: "inconnu"; raison: string };

/** Dossiers repliés (persisté par machine). */
export const collapsedFolders = new Set<string>(
  (() => {
    try {
      return JSON.parse(localStorage.getItem("avash.folders.collapsed") ?? "[]") as string[];
    } catch {
      return [];
    }
  })(),
);
export function saveCollapsed() {
  try {
    localStorage.setItem("avash.folders.collapsed", JSON.stringify([...collapsedFolders]));
  } catch {
    /* stockage indispo */
  }
}

export const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

// ⚠️ `fontFamily` n'appartient PAS au theme : c'est une option du Terminal.
// Place ici, il etait purement ignore et xterm.js retombait sur son defaut
// (courier-new), d'ou un rendu tres laid.
export const THEME_DARK = {
  background: "#0d0f16",
  foreground: "#dfe3ee",
  cursor: "#8b7cf6",
  cursorAccent: "#0d0f16",
  selectionBackground: "rgba(139, 124, 246, .30)",
  selectionForeground: "#ffffff",
  // Palette ANSI : contrastee sur fond sombre, sans saturation criarde.
  black: "#1b1e29", red: "#ef6b73", green: "#4ad295", yellow: "#e8b765",
  blue: "#6e9df8", magenta: "#b98cf7", cyan: "#5fd0e8", white: "#c8cddb",
  brightBlack: "#59617a", brightRed: "#ff8a91", brightGreen: "#71e6ae",
  brightYellow: "#ffd083", brightBlue: "#97bcff", brightMagenta: "#d5adff",
  brightCyan: "#8ce4f7", brightWhite: "#ffffff",
};

/** Thème clair du terminal : fond clair, ANSI assombris pour rester lisibles. */
export const THEME_LIGHT = {
  background: "#f6f7f9",
  foreground: "#1f2430",
  cursor: "#6d5cf0",
  cursorAccent: "#f6f7f9",
  selectionBackground: "rgba(109,92,240,.22)",
  selectionForeground: "#0b0d14",
  black: "#2c3140", red: "#c8353d", green: "#1f9d57", yellow: "#9a6a15",
  blue: "#3059c8", magenta: "#8043c8", cyan: "#0d7d97", white: "#5a6478",
  brightBlack: "#7a8296", brightRed: "#e0555d", brightGreen: "#28b26a",
  brightYellow: "#b3841f", brightBlue: "#4a76e8", brightMagenta: "#9a5fe0",
  brightCyan: "#1596b0", brightWhite: "#1f2430",
};
