// Les styles que xterm.js injecte, rendus effectifs sous la CSP d'Avash.
//
// xterm.js écrit une partie de sa mise en forme dans des éléments <style>
// qu'il ajoute à la page : la police, la taille et la hauteur des lignes du
// rendu DOM, les couleurs du thème, la barre de défilement. La CSP déclare bien
// `style-src 'unsafe-inline'`, mais Tauri y ajoute un nonce à l'exécution, et
// la norme veut qu'un nonce fasse ignorer `'unsafe-inline'` : chacun de ces
// éléments est refusé (violation « style-src-elem inline »). Avec le rendu
// WebGL, qui dessine sur un canevas, rien ne se voyait. Depuis la 0.14.2, les
// terminaux passent au rendu DOM sous Linux (compositing de WebKitGTK coupé,
// voir `terminalWebglPermis`) : ils y perdaient leur police, remplacée par
// celle de l'interface, proportionnelle. Colonnes décalées, art ASCII cassé,
// lignes repliées au mauvais endroit : signalé par le mainteneur le
// 27 novembre 2026 sur l'AppImage, reproduit sur le binaire natif.
//
// Plutôt que d'affaiblir la CSP (retirer le nonce de style-src), on recopie
// le texte de ces éléments dans des feuilles construites (`CSSStyleSheet`),
// adoptées par le document : le CSSOM n'est pas soumis à style-src. Seul le
// CSS que xterm génère à partir de nos propres options passe ainsi, rien qui
// vienne d'un serveur. Les feuilles suivent les éléments : xterm réécrit le
// texte quand la taille ou le thème changent, et retire ses éléments à la
// fermeture du terminal.

type Entree = { feuille: CSSStyleSheet; texte: string | null; obs: MutationObserver };

const suivis = new Map<HTMLStyleElement, Entree>();
/** Toutes les feuilles que ce module a jamais adoptées : celles d'un élément
 *  oublié doivent sortir de `adoptedStyleSheets`, et elles seules. */
const nos = new WeakSet<CSSStyleSheet>();

function prisEnCharge(doc: Document): boolean {
  return "adoptedStyleSheets" in doc && typeof CSSStyleSheet === "function" && "replaceSync" in CSSStyleSheet.prototype;
}

function appliquer(doc: Document): void {
  doc.adoptedStyleSheets = [
    ...doc.adoptedStyleSheets.filter((f) => !nos.has(f)),
    ...[...suivis.values()].map((e) => e.feuille),
  ];
}

function mettreAJour(style: HTMLStyleElement): void {
  const e = suivis.get(style);
  if (!e) return;
  const texte = style.textContent ?? "";
  if (texte === e.texte) return;
  e.texte = texte;
  try {
    e.feuille.replaceSync(texte);
  } catch {
    /* CSS illisible : la feuille reste vide, comme l'élément refusé */
  }
  appliquer(style.ownerDocument);
}

function suivre(style: HTMLStyleElement): void {
  if (suivis.has(style)) return;
  const feuille = new CSSStyleSheet();
  nos.add(feuille);
  const obs = new MutationObserver(() => mettreAJour(style));
  obs.observe(style, { childList: true, characterData: true, subtree: true });
  suivis.set(style, { feuille, texte: null, obs });
  mettreAJour(style);
}

function oublier(style: HTMLStyleElement): void {
  const e = suivis.get(style);
  if (!e) return;
  e.obs.disconnect();
  suivis.delete(style);
  appliquer(style.ownerDocument);
}

/** Reflète les <style> de xterm.js sous `element` (le `term.element` d'un
 *  terminal ouvert) dans des feuilles adoptées. Rend la fonction qui cesse de
 *  les suivre, à appeler à la fermeture du terminal.
 *
 *  Seuls les parents directs des <style> sont observés, sans descendre dans
 *  le reste : sous le rendu DOM, les lignes du terminal changent à chaque
 *  image, et observer tout le sous-arbre ferait payer chaque octet affiché. */
export function refleterStylesXterm(element: HTMLElement): () => void {
  const doc = element.ownerDocument;
  if (!prisEnCharge(doc)) return () => {};
  const scanner = (): void => {
    for (const s of element.querySelectorAll("style")) suivre(s);
    for (const s of [...suivis.keys()]) if (!s.isConnected) oublier(s);
  };
  scanner();
  // L'élément lui-même et l'écran (où le rendu DOM pose ses styles, y compris
  // quand il remplace un rendu WebGL dont le contexte s'est perdu), plus les
  // parents des styles déjà là.
  const parents = new Set<Element>([element]);
  const ecran = element.querySelector(".xterm-screen");
  if (ecran) parents.add(ecran);
  for (const s of element.querySelectorAll("style")) if (s.parentElement) parents.add(s.parentElement);
  const obs = new MutationObserver(scanner);
  for (const p of parents) obs.observe(p, { childList: true });
  const cesser = (): void => {
    obs.disconnect();
    arrets.delete(element);
    for (const s of [...suivis.keys()]) if (element.contains(s) || !s.isConnected) oublier(s);
  };
  arrets.set(element, cesser);
  return cesser;
}

const arrets = new WeakMap<HTMLElement, () => void>();

/** Cesse de refléter les styles du terminal dont `element` est la racine
 *  (sans effet s'il n'était pas suivi). À appeler avant `term.dispose()`. */
export function cesserStylesXterm(element: HTMLElement | undefined): void {
  if (element) arrets.get(element)?.();
}
