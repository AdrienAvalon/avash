// Trouvé par l'audit du 7 septembre 2026 : la modale « Copier vers un autre
// hôte » remplissait son select avec `[...sessions.values()].filter(x => x !== s)`,
// donc aussi les ports série (`serie`, sans SFTP) et les onglets morts (`closed`,
// restés dans le magasin jusqu'à leur fermeture). Le libellé promettait « un
// onglet SSH ouvert » mais la copie n'échouait qu'après lancement, dans la ligne
// de transfert (« Pas de SFTP sur un port série. », « Session N inconnue »).
// Ces tests verrouillent le filtre : seule une session SSH vivante ressort.
import { describe, it, expect } from "vitest";
import { ciblesDeCopie, type Session } from "./etat";

/** Session minimale : seuls id/serie/closed comptent pour le filtre. */
function faireSession(id: number, opts: { serie?: boolean; closed?: boolean } = {}): Session {
  return { id, serie: opts.serie, closed: opts.closed ?? false } as unknown as Session;
}

describe("ciblesDeCopie", () => {
  it("ne garde qu'un onglet SSH vivant parmi une série et un onglet mort", () => {
    const source = faireSession(1);
    const vivante = faireSession(2);
    const serie = faireSession(3, { serie: true });
    const morte = faireSession(4, { closed: true });
    const sessions = new Map<number, Session>([
      [1, source],
      [2, vivante],
      [3, serie],
      [4, morte],
    ]);
    const cibles = ciblesDeCopie(source, sessions);
    expect(cibles).toEqual([vivante]);
  });

  it("exclut la session source elle-même", () => {
    const source = faireSession(1);
    const sessions = new Map<number, Session>([[1, source]]);
    expect(ciblesDeCopie(source, sessions)).toEqual([]);
  });

  it("renvoie une liste vide quand toutes les autres sont série ou mortes", () => {
    // La modale bascule alors sur le message sftp-aucune-autre-session.
    const source = faireSession(1);
    const sessions = new Map<number, Session>([
      [1, source],
      [2, faireSession(2, { serie: true })],
      [3, faireSession(3, { closed: true })],
    ]);
    expect(ciblesDeCopie(source, sessions)).toEqual([]);
  });
});
