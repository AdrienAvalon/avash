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
