// Logique pure du front, extraite de main.ts pour etre testable sans DOM.

export type Host = {
  alias: string;
  hostname: string | null;
  user: string | null;
  port: number | null;
  identity_file: string | null;
  proxy_jump: string | null;
  tags: string[];
};

/** Taille lisible : 1024 -> "1.0 Ko". */
export function humanSize(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  if (n < 1024) return `${n} o`;
  const u = ["K", "M", "G", "T", "P"];
  let i = -1;
  let v = n;
  do {
    v /= 1024;
    i++;
  } while (v >= 1024 && i < u.length - 1);
  return `${v.toFixed(1)} ${u[i]}o`;
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
    (h.user ?? "").toLowerCase().includes(q)
  );
}

export function filterHosts(hosts: Host[], query: string): Host[] {
  return hosts.filter((h) => matchHost(h, query));
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
export const PASSWORD_REQUIRED = "[AVASH_PASSWORD_REQUIRED]";

/** L'echec de connexion tient-il seulement a un mot de passe manquant ? */
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

/** Icone par type de fichier — un repere visuel, pas une taxonomie. */
export function fileIcon(name: string, isDir: boolean): string {
  if (isDir) return "📁";
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "";
  if (/^(png|jpe?g|gif|webp|svg|bmp|ico|heic)$/.test(ext)) return "🖼️";
  if (/^(zip|tar|gz|tgz|bz2|xz|zst|7z|rar|deb|rpm)$/.test(ext)) return "📦";
  if (/^(mp4|mkv|webm|mov|avi|mp3|flac|ogg|wav)$/.test(ext)) return "🎞️";
  if (/^(sh|bash|zsh|fish|py|rs|js|ts|go|c|h|cpp|java|rb|php|lua|toml|ya?ml|json|xml|html|css|sql)$/.test(ext)) return "📜";
  if (/^(pdf|docx?|odt|xlsx?|pptx?)$/.test(ext)) return "📕";
  if (/^(key|pem|pub|crt|gpg|asc)$/.test(ext) || name.startsWith("id_")) return "🔑";
  if (/^(log|txt|md|conf|cfg|ini|env)$/.test(ext) || name.startsWith(".")) return "📄";
  return "📄";
}

/** Date courte : aujourd'hui → « 14:07 », sinon « 12/03/26 ». */
export function shortDate(epochSec: number | null, now = new Date()): string {
  if (!epochSec) return "";
  const d = new Date(epochSec * 1000);
  const sameDay = d.toDateString() === now.toDateString();
  const two = (n: number) => String(n).padStart(2, "0");
  if (sameDay) return `${two(d.getHours())}:${two(d.getMinutes())}`;
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
