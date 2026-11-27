import { describe, it, expect } from "vitest";
import { creerEcrivainOrdonne } from "./ecriture-ordonnee";

/** Un envoi dont on décide quand il aboutit, pour rejouer les courses. */
function envoiPilote() {
  const recus: string[] = [];
  const enCours: { data: string; finir: () => void; echouer: (e: unknown) => void }[] = [];
  const envoyer = (data: string) =>
    new Promise<void>((resolve, reject) => {
      enCours.push({
        data,
        finir: () => {
          recus.push(data);
          resolve();
        },
        echouer: reject,
      });
    });
  return { recus, enCours, envoyer };
}

const tour = () => new Promise((r) => setTimeout(r, 0));

describe("écriture ordonnée vers le PTY", () => {
  it("un seul envoi en vol : la frappe suivante attend la précédente", async () => {
    const p = envoiPilote();
    const ecrire = creerEcrivainOrdonne(p.envoyer, () => {});
    ecrire("e");
    ecrire("c");
    await tour();
    // Avant le correctif, chaque frappe partait par son propre appel, traité
    // en parallèle par le cœur : sous charge « echo » arrivait « cho » ou
    // « ehco » (suite bout en bout Linux, 27 novembre 2026).
    expect(p.enCours.map((e) => e.data)).toEqual(["e"]);
    p.enCours[0].finir();
    await tour();
    expect(p.enCours.map((e) => e.data)).toEqual(["e", "c"]);
  });

  it("les frappes arrivées pendant un envoi partent ensemble, dans l'ordre", async () => {
    const p = envoiPilote();
    const ecrire = creerEcrivainOrdonne(p.envoyer, () => {});
    for (const c of "echo bonjour\r") ecrire(c);
    await tour();
    p.enCours[0].finir();
    await tour();
    p.enCours[1].finir();
    await tour();
    expect(p.recus).toEqual(["e", "cho bonjour\r"]);
    expect(p.recus.join("")).toBe("echo bonjour\r");
  });

  it("un envoi en échec est signalé et n'arrête pas les suivants", async () => {
    const p = envoiPilote();
    const erreurs: unknown[] = [];
    const ecrire = creerEcrivainOrdonne(p.envoyer, (e: unknown) => erreurs.push(e));
    ecrire("a");
    ecrire("b");
    await tour();
    p.enCours[0].echouer(new Error("Session 7 inconnue"));
    await tour();
    expect(erreurs).toHaveLength(1);
    p.enCours[1].finir();
    await tour();
    expect(p.recus).toEqual(["b"]);
    // L'écrivain reste utilisable après un échec.
    ecrire("c");
    await tour();
    p.enCours[2].finir();
    await tour();
    expect(p.recus).toEqual(["b", "c"]);
  });

  it("au repos, une frappe part aussitôt, sans attendre de lot", () => {
    const p = envoiPilote();
    const ecrire = creerEcrivainOrdonne(p.envoyer, () => {});
    ecrire("x");
    expect(p.enCours.map((e) => e.data)).toEqual(["x"]);
  });
});
