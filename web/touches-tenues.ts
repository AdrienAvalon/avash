// Suivi des touches (et boutons souris) tenus enfoncés dans UNE session de
// bureau distant, pour tout relâcher quand le canvas perd le focus.
//
// Trouvé par l'audit du 7 septembre 2026 : le canvas envoyait chaque keydown /
// keyup tel quel, sans jamais écouter blur ni visibilitychange. Si le keyup
// arrivait après que le canvas eut perdu le focus (Alt+Tab, touche Super, ou
// Ctrl+Tab — le propre raccourci d'onglet d'Avash), la touche restait tenue
// côté bureau distant : les clics ouvraient des menus, les frappes devenaient
// des raccourcis, jusqu'à ce que l'utilisateur réappuie puis relâche la touche
// fautive. On mémorise donc ce qui est enfoncé pour pouvoir tout relâcher au
// blur et à la mise en arrière-plan de l'onglet.

/** Un jeton de touche : `KeyboardEvent.code` en RDP, le keysym (en chaîne) en VNC. */
export class ToucheTenues {
  private readonly tenues = new Set<string>();
  /** Construit le message de relâchement d'un jeton, ou `null` s'il n'a pas de
   *  correspondance (touche non prise en charge par le protocole). */
  private readonly messageRelache: (jeton: string) => number[] | null;

  constructor(messageRelache: (jeton: string) => number[] | null) {
    this.messageRelache = messageRelache;
  }

  /** Marque une touche comme enfoncée dans cette session. */
  enfoncer(jeton: string): void {
    this.tenues.add(jeton);
  }

  /** Retire une touche ; renvoie vrai si elle était bien tenue dans CETTE
   *  session. Faux quand un keyup arrive sans keydown préalable (le keyup de
   *  Ctrl après un Ctrl+Tab atterrit sur le NOUVEAU canvas) : l'appelant ne doit
   *  alors rien transmettre, sinon le serveur VNC reçoit un release fantôme
   *  d'une touche jamais pressée chez lui. */
  relacher(jeton: string): boolean {
    return this.tenues.delete(jeton);
  }

  /** Messages de relâchement pour tout ce qui est tenu, puis vide l'ensemble. */
  relacherTout(): number[][] {
    const msgs: number[][] = [];
    for (const jeton of this.tenues) {
      const m = this.messageRelache(jeton);
      if (m) msgs.push(m);
    }
    this.tenues.clear();
    return msgs;
  }
}
