// Jeu d'icônes SVG cohérent (trait fin, grille 24, currentColor) — remplace
// les emoji du châssis, qui rendaient différemment selon l'OS. Style dérivé de
// Lucide (ISC). Les chaînes sont des constantes internes : les insérer via
// innerHTML est sûr.

const P: Record<string, string> = {
  search: '<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  key: '<circle cx="7.5" cy="15.5" r="4.5"/><path d="m10.5 12.5 8-8M16 6l2 2M19 3l2 2"/>',
  tunnel: '<path d="M8 3 4 7l4 4"/><path d="M4 7h12a4 4 0 0 1 0 8h-1"/><path d="m16 21 4-4-4-4"/><path d="M20 17H9"/>',
  zap: '<path d="M13 2 4.5 13.5H11l-1 8.5L18.5 10.5H12z"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  file: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>',
  fileCode: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/><path d="m10 12-2 2 2 2M14 12l2 2-2 2"/>',
  fileArchive: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/><path d="M11 9v1M11 12v1M11 15v1"/>',
  image: '<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="8.5" cy="9.5" r="1.5"/><path d="m21 16-4.5-4.5L6 22"/>',
  film: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 4v16M17 4v16M3 9h4M3 15h4M17 9h4M17 15h4"/>',
  book: '<path d="M4 5a2 2 0 0 1 2-2h13v16H6a2 2 0 0 0-2 2z"/><path d="M4 19a2 2 0 0 0 2 2h13"/>',
  cornerUpLeft: '<path d="M9 14 4 9l5-5"/><path d="M4 9h11a4 4 0 0 1 4 4v7"/>',
  arrowUp: '<path d="M12 19V5M5 12l7-7 7 7"/>',
  arrowDown: '<path d="M12 5v14M19 12l-7 7-7-7"/>',
  refresh: '<path d="M21 12a9 9 0 1 1-2.6-6.4L21 8"/><path d="M21 3v5h-5"/>',
  upload: '<path d="M12 15V3M7 8l5-5 5 5"/><path d="M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>',
  download: '<path d="M12 3v12M7 10l5 5 5-5"/><path d="M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>',
  play: '<path d="M7 4v16l13-8z"/>',
  stop: '<rect x="6" y="6" width="12" height="12" rx="1.5"/>',
  pencil: '<path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/>',
  trash: '<path d="M4 7h16M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2M6 7l1 13a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1l1-13"/>',
  x: '<path d="M6 6l12 12M18 6 6 18"/>',
  copy: '<rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h8"/>',
  terminal: '<path d="M5 5 11 12l-6 7"/><path d="M13 19h6"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
  moon: '<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/>',
  monitor: '<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/>',
  winMin: '<path d="M5 12h14"/>',
  winMax: '<rect x="5" y="5" width="14" height="14" rx="1.5"/>',
  winRestore: '<rect x="8" y="8" width="11" height="11" rx="1.5"/><path d="M5 15V6a1 1 0 0 1 1-1h9"/>',
  winClose: '<path d="M6 6l12 12M18 6 6 18"/>',
};

/** Renvoie une icône SVG inline. `cls` s'ajoute à la classe `ic-svg`. */
// `string & {}` accepte n'importe quelle chaîne tout en gardant l'autocomplétion
// des clés connues (idiome TS) : sans lui, `keyof typeof P` serait absorbé par
// `string` et le lint le signalerait comme doublon.
export function ic(name: keyof typeof P | (string & {}), cls = ""): string {
  const body = P[name] ?? P.file;
  return `<svg class="ic-svg ${cls}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
}

/** Icône d'un fichier d'après son nom — clés du jeu ci-dessus. */
export function fileIconName(name: string, isDir: boolean): string {
  if (isDir) return "folder";
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "";
  if (/^(png|jpe?g|gif|webp|svg|bmp|ico|heic)$/.test(ext)) return "image";
  if (/^(zip|tar|gz|tgz|bz2|xz|zst|7z|rar|deb|rpm)$/.test(ext)) return "fileArchive";
  if (/^(mp4|mkv|webm|mov|avi|mp3|flac|ogg|wav)$/.test(ext)) return "film";
  if (/^(sh|bash|zsh|fish|py|rs|js|ts|go|c|h|cpp|java|rb|php|lua|toml|ya?ml|json|xml|html|css|sql)$/.test(ext)) return "fileCode";
  if (/^(pdf|docx?|odt|xlsx?|pptx?)$/.test(ext)) return "book";
  if (/^(key|pem|pub|crt|gpg|asc)$/.test(ext) || name.startsWith("id_")) return "key";
  return "file";
}


/** Remplace les marqueurs `[data-icon]` d'un arbre DOM par des SVG.
 *  Pur DOM (aucune dépendance Tauri) — appelé au démarrage, testable. */
export function hydrateIcons(root: ParentNode = document): void {
  for (const el of root.querySelectorAll<HTMLElement>("[data-icon]")) {
    const name = el.dataset.icon ?? "file";
    const label = el.textContent?.trim() ?? "";
    el.textContent = "";
    if (el.classList.contains("manual-btn")) {
      const box = document.createElement("span");
      box.className = "ic";
      box.innerHTML = ic(name);
      const txt = document.createElement("span");
      txt.textContent = label;
      el.append(box, txt);
    } else {
      el.innerHTML = ic(name);
      if (label) {
        const txt = document.createElement("span");
        txt.textContent = label;
        el.append(txt);
      }
    }
  }
}
