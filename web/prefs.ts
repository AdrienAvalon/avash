// Réglages retenus d'un lancement à l'autre.
//
// Isolés du module d'entrée pour être exerçables par les tests : ils décident de
// ce qui sort de la machine, ce qui les rend trop importants pour n'être
// couverts que de bout en bout.

/** Le presse-papiers est-il échangé avec les bureaux distants ?
 *
 *  Le réglage vaut dans les **deux** sens. Sortant : confier le contenu du
 *  presse-papiers — souvent un mot de passe qu'on vient de copier — à un
 *  serveur qui peut le réclamer dès qu'on le lui annonce. Entrant : laisser un
 *  bureau distant remplacer le presse-papiers du poste, ce dont un serveur
 *  hostile se sert pour faire coller autre chose que ce qu'on croit avoir
 *  copié. C'est le comportement attendu d'un client RDP, donc le défaut, mais
 *  il doit rester révocable : la bascule est offerte à la palette (Ctrl+K).
 *  L'absence de réglage vaut « partagé ». */
export const CLIP_KEY = "avash.rdp.clipboard";

export function partageClipboard(): boolean {
  // Un navigateur peut refuser l'accès au stockage (mode privé strict) : on
  // retombe alors sur le défaut plutôt que de faire échouer l'appelant.
  try {
    return localStorage.getItem(CLIP_KEY) !== "0";
  } catch {
    return true;
  }
}

export function setPartageClipboard(actif: boolean): void {
  try {
    localStorage.setItem(CLIP_KEY, actif ? "1" : "0");
  } catch {
    /* stockage indisponible : le réglage vaut pour la session en cours */
  }
}

/** Sonder la santé des hôtes au démarrage : coupé par défaut — une sonde
 *  ouvre une connexion vers chaque serveur, ce n'est pas anodin partout. */
export const SANTE_DEMARRAGE_KEY = "avash.sante.demarrage";

export function sondeAuDemarrage(): boolean {
  try {
    return localStorage.getItem(SANTE_DEMARRAGE_KEY) === "1";
  } catch {
    return false;
  }
}

export function setSondeAuDemarrage(actif: boolean): void {
  try {
    localStorage.setItem(SANTE_DEMARRAGE_KEY, actif ? "1" : "0");
  } catch {
    /* stockage indisponible : le choix vaut pour la session */
  }
}
