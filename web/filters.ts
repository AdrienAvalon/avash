// Logique pure du front, extraite de main.ts pour etre testable sans DOM.

export type Host = {
  alias: string;
  hostname: string | null;
  user: string | null;
  port: number | null;
  identity_file: string | null;
  proxy_jump: string | null;
  tags: string[];
  folder: string;
};

/** Taille lisible : 1024 -> "1.0 Ko". */
export function humanSize(n: number, langue: "fr" | "en" = "fr"): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  const octet = langue === "fr" ? "o" : "B";
  if (n < 1024) return `${n} ${octet}`;
  const u = ["K", "M", "G", "T", "P"];
  let i = -1;
  let v = n;
  do {
    v /= 1024;
    i++;
  } while (v >= 1024 && i < u.length - 1);
  return `${v.toFixed(1)} ${u[i]}${octet}`;
}

/**
 * Un hote correspond-il a la recherche ?
 *
 * La barre laterale et la palette filtraient differemment : l'une sur
 * alias + hostname, l'autre sur l'alias seul. Taper une adresse IP dans la
 * palette ne trouvait donc rien. Meme regle partout desormais.
 */
export function matchHost(h: Host, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return (
    h.alias.toLowerCase().includes(q) ||
    (h.hostname ?? "").toLowerCase().includes(q) ||
    (h.user ?? "").toLowerCase().includes(q) ||
    h.tags.some((t) => t.toLowerCase().includes(q))
  );
}

/** Tous les tags presents, dedupliques, tries. */
export function allTags(hosts: Host[]): string[] {
  const set = new Set<string>();
  for (const h of hosts) for (const t of h.tags) set.add(t);
  return [...set].sort((a, b) => a.localeCompare(b));
}

export function filterHosts(hosts: Host[], query: string, tag: string | null = null): Host[] {
  return hosts.filter((h) => matchHost(h, query) && (!tag || h.tags.includes(tag)));
}

/**
 * Assemble un chemin distant. La logique `path === "/" ? ... : ...` etait
 * recopiee a trois endroits ; une seule version evite qu'elles divergent.
 */
export function remoteJoin(path: string, name: string): string {
  const base = path.endsWith("/") ? path.slice(0, -1) : path;
  return `${base}/${name}`;
}

/**
 * Répertoire parent d'un chemin distant, pour la remontée « .. » du SFTP.
 *
 * La regex inline `path.replace(/\/[^/]+\/?$/, "") || "/"` était correcte mais
 * illisible et non testée. Les cas limites (racine, slash final, chemin à un
 * seul segment) sont exactement là où ce genre de code casse.
 */
export function parentDir(path: string): string {
  if (path === "/" || path === "") return "/";
  const trimmed = path.endsWith("/") ? path.slice(0, -1) : path;
  const cut = trimmed.lastIndexOf("/");
  if (cut <= 0) return "/";
  return trimmed.slice(0, cut);
}

/** Marqueur pose par le backend quand seul le mot de passe manque. */
const PASSWORD_REQUIRED = "[AVASH_PASSWORD_REQUIRED]";

/** L'echec de connexion tient-il seulement a un mot de passe manquant ? */
export function isHostKeyChanged(errorMessage: string): boolean {
  return errorMessage.includes("[AVASH_HOST_KEY_CHANGED]");
}

export function isPasswordRequired(errorMessage: string): boolean {
  return errorMessage.includes(PASSWORD_REQUIRED);
}

/**
 * Retire les caractères qui permettraient d'injecter du HTML.
 *
 * Utilisé partout où du texte non maîtrisé (filtre de recherche) est inséré
 * via innerHTML. Une chaîne de recherche contenant `<img onerror=...>`
 * exécuterait du code dans la webview, qui a accès à `invoke`.
 */
export function stripHtml(text: string): string {
  return text.replace(/[<>&]/g, "");
}

// ---------- Tunnels ----------

export type TunnelKind = "local" | "remote" | "dynamic";

export type TunnelDef = {
  id: string;
  alias: string;
  kind: TunnelKind;
  bind_port: number;
  target_host: string;
  target_port: number;
  name: string;
};

export type TunnelStatus = {
  id: string;
  bound_port: number;
  active: number;
  total: number;
  bytes_up: number;
  bytes_down: number;
  alive: boolean;
  last_error: string | null;
};

/** Lettre `ssh` correspondante : repere familier pour qui connait la ligne de commande. */
export function tunnelFlag(kind: TunnelKind): string {
  return { local: "-L", remote: "-R", dynamic: "-D" }[kind];
}

/** Resume dans le sens du trafic, identique a `TunnelDef::describe` cote Rust. */
export function describeTunnel(d: TunnelDef): string {
  switch (d.kind) {
    case "local":
      return `localhost:${d.bind_port} → ${d.alias} → ${d.target_host}:${d.target_port}`;
    case "remote":
      return `${d.alias}:${d.bind_port} → localhost → ${d.target_host}:${d.target_port}`;
    case "dynamic":
      return `SOCKS5 localhost:${d.bind_port} → ${d.alias}`;
  }
}

/** « 2 conn · ↑1.0 Ko ↓3.4 Ko » — compact, pour une ligne de liste. */
export function tunnelTraffic(s: TunnelStatus): string {
  const conn = s.active > 0 ? `${s.active} conn` : `${s.total} au total`;
  return `${conn} · ↑${humanSize(s.bytes_up)} ↓${humanSize(s.bytes_down)}`;
}

/** Nombre de tunnels vivants par hote, pour le badge de la barre laterale. */
export function activeTunnelsByHost(defs: TunnelDef[], status: Map<string, TunnelStatus>): Map<string, number> {
  const out = new Map<string, number>();
  for (const d of defs) {
    if (status.get(d.id)?.alive) out.set(d.alias, (out.get(d.alias) ?? 0) + 1);
  }
  return out;
}

// ---------- Avatar d'hote ----------

/** Deux caracteres pour l'avatar : « prod-web » → « PW », « 10.0.0.7 » → « 10 ». */
export function hostInitials(alias: string): string {
  const parts = alias.split(/[^a-z0-9]+/i).filter(Boolean);
  if (parts.length >= 2 && /[a-z]/i.test(parts[0]) && /[a-z]/i.test(parts[1])) {
    return (parts[0][0] + parts[1][0]).toUpperCase();
  }
  return alias.replace(/[^a-z0-9]/gi, "").slice(0, 2).toUpperCase() || "?";
}

/** Couleur stable par nom : le meme hote garde la meme teinte a chaque lancement. */
export function hostHue(alias: string): string {
  let h = 0;
  for (const c of alias) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return `hsl(${h % 360} 60% 62%)`;
}

// ---------- Logo de distribution ----------

export type OsInfo = { id: string; like: string[]; pretty: string };

/**
 * Glyphes « Font Logos » de la Nerd Font embarquee (U+F300…), couleur de
 * marque a cote. Une distribution inconnue retombe sur sa famille (ID_LIKE),
 * puis sur Tux.
 */
const OS_BADGES: Record<string, { glyph: string; color: string }> = {
  alpine: { glyph: "", color: "#0d597f" },
  darwin: { glyph: "", color: "#c9ced6" },
  macos: { glyph: "", color: "#c9ced6" },
  arch: { glyph: "", color: "#1793d1" },
  archlinux: { glyph: "", color: "#1793d1" },
  centos: { glyph: "", color: "#9ccd2a" },
  debian: { glyph: "", color: "#d70a53" },
  deepin: { glyph: "", color: "#2ca7f8" },
  devuan: { glyph: "", color: "#7f8fa6" },
  elementary: { glyph: "", color: "#64baff" },
  fedora: { glyph: "", color: "#51a2da" },
  freebsd: { glyph: "", color: "#ab2b28" },
  gentoo: { glyph: "", color: "#a79cd0" },
  linuxmint: { glyph: "", color: "#87cf3e" },
  mageia: { glyph: "", color: "#2397d4" },
  manjaro: { glyph: "", color: "#35bf5c" },
  nixos: { glyph: "", color: "#7ebae4" },
  opensuse: { glyph: "", color: "#73ba25" },
  "opensuse-leap": { glyph: "", color: "#73ba25" },
  "opensuse-tumbleweed": { glyph: "", color: "#73ba25" },
  suse: { glyph: "", color: "#73ba25" },
  raspbian: { glyph: "", color: "#c51a4a" },
  rhel: { glyph: "", color: "#ee0000" },
  redhat: { glyph: "", color: "#ee0000" },
  slackware: { glyph: "", color: "#4a90d9" },
  ubuntu: { glyph: "", color: "#e95420" },
  almalinux: { glyph: "", color: "#4cb4e3" },
  artix: { glyph: "", color: "#10a0cc" },
  endeavouros: { glyph: "", color: "#7f7fff" },
  kali: { glyph: "", color: "#6ea0c8" },
  openbsd: { glyph: "", color: "#f2ca30" },
  pop: { glyph: "", color: "#48b9c7" },
  rocky: { glyph: "", color: "#10b981" },
  void: { glyph: "", color: "#478061" },
  windows: { glyph: "", color: "#0078d4" },
  linux: { glyph: "", color: "#e8b765" },
  bsd: { glyph: "", color: "#ab2b28" },
};

export function osBadge(os: OsInfo): { glyph: string; color: string } {
  for (const key of [os.id, ...os.like]) {
    const b = OS_BADGES[key];
    if (b) return b;
  }
  return OS_BADGES.linux;
}


// ---------- SFTP ----------

export type SftpEntry = { name: string; is_dir: boolean; size: number; modified: number | null };

/** Tri d'affichage : dossiers d'abord, puis ordre alphabétique. */
export function sortSftpEntries(entries: SftpEntry[]): SftpEntry[] {
  return [...entries].sort(
    (a, b) => (b.is_dir ? 1 : 0) - (a.is_dir ? 1 : 0) || a.name.localeCompare(b.name),
  );
}

/** Date courte : aujourd'hui → « 14:07 », sinon « 12/03/26 ». */
export function shortDate(epochSec: number | null, now = new Date(), langue: "fr" | "en" = "fr"): string {
  if (!epochSec) return "";
  const d = new Date(epochSec * 1000);
  const sameDay = d.toDateString() === now.toDateString();
  const two = (n: number) => String(n).padStart(2, "0");
  if (sameDay) return `${two(d.getHours())}:${two(d.getMinutes())}`;
  // En anglais, la forme ISO : « 03/12 » se lirait dans les deux sens.
  if (langue === "en") return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`;
  return `${two(d.getDate())}/${two(d.getMonth() + 1)}/${two(d.getFullYear() % 100)}`;
}

/** Cite un chemin pour un shell POSIX : `'a b'` , apostrophe echappee. */
export function shellQuote(path: string): string {
  return "'" + path.replace(/'/g, "'\\''") + "'";
}

/** Refuse les noms qui sortiraient du dossier ou seraient invalides. */
export function validFileName(name: string): boolean {
  return name.length > 0 && name !== "." && name !== ".." && !name.includes("/") && !name.includes("\0");
}

// ---------- Snippets ----------

export type Snippet = { id: string; name: string; command: string; run: boolean; category: string };

/** Apercu sur une ligne : saut de ligne visible, tronque proprement. */
export function snippetPreview(command: string, max = 60): string {
  const oneLine = command.replace(/\r?\n/g, " ⏎ ").trim();
  return oneLine.length > max ? oneLine.slice(0, max - 1) + "…" : oneLine;
}

/** Variables {{nom}} d'une commande, sans doublon, dans l'ordre — miroir du
 *  Rust pour un apercu instantane cote formulaire. */
export function snippetVars(command: string): string[] {
  const out: string[] = [];
  const re = /\{\{\s*([^}]*?)\s*\}\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(command)) !== null) {
    const name = m[1].trim();
    if (name && !out.includes(name)) out.push(name);
  }
  return out;
}

/** Substitue {{nom}} par sa valeur ; variable absente → chaine vide. */
export function renderSnippet(command: string, vars: Record<string, string>): string {
  return command.replace(/\{\{\s*([^}]*?)\s*\}\}/g, (_, name) => vars[name.trim()] ?? "");
}

// ---------- Arborescence des hôtes (dossiers unifiés SSH + RDP) ----------

/**
 * Un nœud de l'arbre des hôtes : un dossier, ses sous-dossiers, et les éléments
 * qu'il contient directement. Le type des éléments (`T`) est libre — côté UI ce
 * sont des hôtes SSH ou des bureaux RDP —, ce qui rend cette logique testable
 * sans dépendre du DOM ni de l'état global.
 */
export type FolderNode<T> = {
  name: string;
  path: string;
  children: Map<string, FolderNode<T>>;
  items: T[];
};

function newFolderNode<T>(name: string, path: string): FolderNode<T> {
  return { name, path, children: new Map(), items: [] };
}

/**
 * Descend dans l'arbre jusqu'au nœud du chemin `a/b/c`, en créant les nœuds
 * manquants au passage. `""` (ou un chemin vide) renvoie la racine.
 */
export function ensureFolderNode<T>(root: FolderNode<T>, path: string): FolderNode<T> {
  let node = root;
  let acc = "";
  for (const seg of (path || "").split("/").filter(Boolean)) {
    acc = acc ? `${acc}/${seg}` : seg;
    let child = node.children.get(seg);
    if (!child) {
      child = newFolderNode<T>(seg, acc);
      node.children.set(seg, child);
    }
    node = child;
  }
  return node;
}

/**
 * Construit l'arbre à partir des dossiers connus (le registre, qui retient
 * aussi les dossiers VIDES) et des éléments rangés (chacun avec son chemin de
 * dossier). L'arbre est l'union des dossiers du registre et de ceux référencés
 * par les éléments — un dossier peut donc apparaître même si le registre l'ignore.
 */
export function buildFolderTree<T>(
  folders: string[],
  items: { folder: string; item: T }[],
): FolderNode<T> {
  const root = newFolderNode<T>("", "");
  for (const f of folders) ensureFolderNode(root, f);
  for (const { folder, item } of items) ensureFolderNode(root, folder || "").items.push(item);
  return root;
}

/** Nombre total d'éléments dans un nœud et tous ses descendants (récursif). */
/** Tous les chemins de dossiers à proposer : ceux du registre, plus chaque
 *  préfixe des dossiers réellement portés par des hôtes (`a/b/c` ⇒ `a`, `a/b`,
 *  `a/b/c`), sans doublon, triés. Un hôte rangé dans un dossier que le
 *  registre ignore doit tout de même le faire apparaître. */
export function allFolderPaths(registered: string[], used: string[]): string[] {
  const set = new Set<string>(registered);
  for (const f of used) {
    let acc = "";
    for (const seg of (f || "").split("/").filter(Boolean)) {
      acc = acc ? `${acc}/${seg}` : seg;
      set.add(acc);
    }
  }
  return [...set].filter(Boolean).sort();
}

export function folderNodeCount<T>(node: FolderNode<T>): number {
  let n = node.items.length;
  for (const c of node.children.values()) n += folderNodeCount(c);
  return n;
}

// ---------- Entrées RDP (scancodes clavier, mappage souris) ----------

/**
 * Traduit un `KeyboardEvent.code` en scancode PC/XT (set 1), attendu par le
 * protocole RDP. Renvoie `null` pour une touche non prise en charge.
 */
export function rdpScancode(code: string): number | null {
  const map: Record<string, number> = {
    // --- Bloc alphanumérique et modificateurs (jeu de scancodes 1) ---
    Escape: 0x01, Digit1: 0x02, Digit2: 0x03, Digit3: 0x04, Digit4: 0x05, Digit5: 0x06,
    Digit6: 0x07, Digit7: 0x08, Digit8: 0x09, Digit9: 0x0a, Digit0: 0x0b, Minus: 0x0c, Equal: 0x0d,
    Backspace: 0x0e, Tab: 0x0f, KeyQ: 0x10, KeyW: 0x11, KeyE: 0x12, KeyR: 0x13, KeyT: 0x14,
    KeyY: 0x15, KeyU: 0x16, KeyI: 0x17, KeyO: 0x18, KeyP: 0x19, BracketLeft: 0x1a, BracketRight: 0x1b,
    Enter: 0x1c, ControlLeft: 0x1d, KeyA: 0x1e, KeyS: 0x1f, KeyD: 0x20, KeyF: 0x21, KeyG: 0x22,
    KeyH: 0x23, KeyJ: 0x24, KeyK: 0x25, KeyL: 0x26, Semicolon: 0x27, Quote: 0x28, Backquote: 0x29,
    ShiftLeft: 0x2a, Backslash: 0x2b, KeyZ: 0x2c, KeyX: 0x2d, KeyC: 0x2e, KeyV: 0x2f, KeyB: 0x30,
    KeyN: 0x31, KeyM: 0x32, Comma: 0x33, Period: 0x34, Slash: 0x35, ShiftRight: 0x36,
    AltLeft: 0x38, Space: 0x39, CapsLock: 0x3a,
    // Touche à gauche de Maj sur les claviers européens (« < > » en AZERTY).
    IntlBackslash: 0x56,

    // --- Touches de fonction ---
    F1: 0x3b, F2: 0x3c, F3: 0x3d, F4: 0x3e, F5: 0x3f, F6: 0x40,
    F7: 0x41, F8: 0x42, F9: 0x43, F10: 0x44, F11: 0x57, F12: 0x58,

    // --- Pavé numérique (verrou compris) ---
    NumLock: 0x45, ScrollLock: 0x46,
    NumpadMultiply: 0x37, NumpadSubtract: 0x4a, NumpadAdd: 0x4e,
    Numpad7: 0x47, Numpad8: 0x48, Numpad9: 0x49,
    Numpad4: 0x4b, Numpad5: 0x4c, Numpad6: 0x4d,
    Numpad1: 0x4f, Numpad2: 0x50, Numpad3: 0x51,
    Numpad0: 0x52, NumpadDecimal: 0x53,

    // --- Touches « étendues » : préfixe 0xE0, décodé par le sidecar ---
    // AltGr en fait partie. Sans elle, tous les caractères de troisième niveau
    // d'un clavier français sont inaccessibles — dont l'antislash (AltGr+8).
    AltRight: 0xe038, ControlRight: 0xe01d,
    NumpadEnter: 0xe01c, NumpadDivide: 0xe035,
    Home: 0xe047, ArrowUp: 0xe048, PageUp: 0xe049,
    ArrowLeft: 0xe04b, ArrowRight: 0xe04d,
    End: 0xe04f, ArrowDown: 0xe050, PageDown: 0xe051,
    Insert: 0xe052, Delete: 0xe053,
    MetaLeft: 0xe05b, MetaRight: 0xe05c, ContextMenu: 0xe05d,
  };
  return map[code] ?? null;
}

/** Encode un entier 16 bits en petit-boutiste (2 octets), pour les messages WS. */
export function le16(n: number): [number, number] {
  return [n & 0xff, (n >> 8) & 0xff];
}

/**
 * Mappe la position d'un clic (coordonnées écran) vers un pixel du bureau RDP.
 * Le canvas est affiché en `object-fit: contain` : l'image est mise à l'échelle
 * pour tenir dans l'élément en gardant son ratio, donc letterboxée (bandes). On
 * retrouve le rectangle réellement peint pour un mappage exact, borné au bureau.
 */
export function rdpMousePos(
  clientX: number,
  clientY: number,
  rect: { left: number; top: number; width: number; height: number },
  w: number,
  h: number,
): [number, number] {
  const scale = Math.min(rect.width / w, rect.height / h);
  const dispW = w * scale;
  const dispH = h * scale;
  const offX = (rect.width - dispW) / 2;
  const offY = (rect.height - dispH) / 2;
  const x = Math.max(0, Math.min(w - 1, Math.round(((clientX - rect.left - offX) / dispW) * w)));
  const y = Math.max(0, Math.min(h - 1, Math.round(((clientY - rect.top - offY) / dispH) * h)));
  return [x, y];
}

/**
 * Choisit l'état des verrous clavier à imposer au bureau distant.
 *
 * Le système est prioritaire quand il sait répondre (diodes du clavier sous
 * Linux, état des touches sous Windows). Les événements clavier ne sont qu'un
 * secours : **WebKitGTK ne renseigne pas `getModifierState("NumLock")`** — il
 * répond toujours faux, verrou allumé ou non (mesuré). Les laisser primer
 * revenait à éteindre le pavé numérique du distant dès la première frappe.
 *
 * `null` signifie « je ne sais pas » : mieux vaut ne rien imposer au distant
 * que de lui affirmer un état faux.
 */
export function choisirVerrous(systeme: number | null, evenements: number | null): number | null {
  return systeme ?? evenements;
}
