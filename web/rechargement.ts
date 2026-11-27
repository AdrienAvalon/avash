// Gardes contre le rechargement involontaire de l'interface.
//
// Audit du 27 novembre 2026 (WI-1, FI-2, ME-3). Sous WebView2, F5, Ctrl+R et
// Ctrl+Maj+R rechargent la page dès que le focus n'est pas dans un terminal
// (barre latérale, panneau SFTP, où F5 est le réflexe « actualiser » de
// l'Explorateur et de WinSCP) ; sous WebKitGTK comme sous WebView2, le menu
// contextuel par défaut propose « Recharger » sur toute zone sans menu à nous.
// Un rechargement faisait disparaître tous les onglets, et le cœur gardait les
// sessions sans onglet. Le cœur coupe désormais les raccourcis du navigateur
// de WebView2 et ferme les sessions orphelines au chargement ; ces gardes sont
// la ceinture côté page, et la seule défense contre le menu contextuel.

/** La touche demande-t-elle au navigateur de recharger la page ? F5 (avec ou
 *  sans Ctrl ou Maj), Ctrl+R et Ctrl+Maj+R, ou Cmd+R. */
export function estToucheDeRechargement(e: Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "altKey">): boolean {
  if (e.key === "F5" || e.code === "F5") return !e.altKey;
  const mod = e.ctrlKey || e.metaKey;
  if (!mod || e.altKey) return false;
  return e.code === "KeyR" || e.key.toLowerCase() === "r";
}

/** Le menu contextuel natif reste-t-il permis sur cette cible ? Dans un champ
 *  de saisie, il porte couper, copier, coller et annuler, et ne propose pas de
 *  recharger : on le garde. Partout ailleurs, la page n'a que « Recharger »,
 *  « Précédent » ou « Imprimer » à offrir, aucun n'a de sens ici. */
export function menuNatifPermis(cible: EventTarget | null): boolean {
  if (!(cible instanceof Element)) return false;
  const champ = cible.closest("input, textarea, [contenteditable]:not([contenteditable='false'])");
  if (!champ) return false;
  // Le champ caché de xterm est une zone de saisie : le terminal a son propre
  // menu, qui empêche déjà l'action par défaut.
  return !champ.classList.contains("xterm-helper-textarea");
}

/** Pose les deux gardes sur la fenêtre.
 *
 *  La touche : `preventDefault` en phase de capture, **sans** arrêter sa
 *  propagation. Ctrl+R est la recherche inverse de bash, F5 une touche que vim
 *  ou un bureau distant attendent : xterm et le canevas du bureau doivent
 *  encore la recevoir, et ni l'un ni l'autre ne regarde `defaultPrevented`.
 *
 *  Le menu : en phase de bouillonnement, après les menus d'Avash (terminal,
 *  barre latérale, panneau SFTP, bureau distant), qui empêchent déjà l'action
 *  par défaut là où ils s'ouvrent. */
export function installerGardesRechargement(cible: Pick<Window, "addEventListener">): void {
  cible.addEventListener(
    "keydown",
    (e) => {
      if (estToucheDeRechargement(e)) e.preventDefault();
    },
    true,
  );
  cible.addEventListener("contextmenu", (e) => {
    if (e.defaultPrevented || menuNatifPermis(e.target)) return;
    e.preventDefault();
  });
}
