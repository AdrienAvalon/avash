// Machine d'états de la file des transferts SFTP, testée sans DOM.
// Audit du 12 septembre 2026 (couverture.md, section 5, point 3) : la file ne se
// testait qu'à travers la page (trois tests DOM la contournaient). Ces tests
// jouent la file comme sftp.ts la joue : qui part, comment une issue se classe,
// ce que la ligne affiche.
import { describe, it, expect } from "vitest";
import {
  aLancer, appliquerProgression, boutonAnnulerVisible, conclure, detailTransfert, effacableAuClic,
  nouveauTransfert, pourcentage, PARALLELE, type Transfert,
} from "./sftp-transferts";
import { humanSize } from "./filters";
import { langue, t } from "./i18n";

function transfert(id: number, champs: Partial<Transfert> = {}): Transfert {
  return { ...nouveauTransfert(id, "download", `f${id}`, null, () => Promise.resolve("ok")), ...champs };
}

/** Démarre ce que la file permet, comme `planifier` dans sftp.ts. */
function planifier(file: Transfert[]): Transfert[] {
  const partants = aLancer(file);
  for (const x of partants) x.etat = "en-cours";
  return partants;
}

describe("file des transferts : qui part", () => {
  it("trois transferts au plus partent de front, les suivants attendent", () => {
    const file = [1, 2, 3, 4, 5].map((i) => transfert(i));
    expect(planifier(file).map((x) => x.id)).toEqual([1, 2, 3]);
    expect(file.map((x) => x.etat)).toEqual(["en-cours", "en-cours", "en-cours", "attente", "attente"]);
    expect(PARALLELE).toBe(3);
    // Rien de plus ne part tant qu'aucune place ne se libère.
    expect(planifier(file)).toEqual([]);
  });

  it("une fin libère une place pour le premier en attente, dans l'ordre de la file", () => {
    const file = [1, 2, 3, 4, 5].map((i) => transfert(i));
    planifier(file);
    conclure(file[1], { ok: true, message: "fait" });
    expect(planifier(file).map((x) => x.id)).toEqual([4]);
  });

  it("une ligne annulée pendant son attente ne part jamais", () => {
    const file = [1, 2, 3, 4, 5].map((i) => transfert(i));
    planifier(file);
    file[3].etat = "annule"; // clic sur « Annuler » d'une ligne en attente
    conclure(file[0], { ok: false, erreur: "Connexion perdue" });
    expect(planifier(file).map((x) => x.id)).toEqual([5]);
    expect(file[3].etat).toBe("annule");
  });

  it("aucune ligne ne part si la file est vide ou toute terminée", () => {
    expect(aLancer([])).toEqual([]);
    expect(aLancer([transfert(1, { etat: "fini" }), transfert(2, { etat: "erreur" })])).toEqual([]);
  });
});

describe("file des transferts : issue d'un transfert", () => {
  it("un succès remplit la barre même sans événement de progression", () => {
    const x = transfert(1, { etat: "en-cours", fait: 40, total: 100 });
    conclure(x, { ok: true, message: "3 fichiers reçus" });
    expect(x.etat).toBe("fini");
    expect(x.fait).toBe(100);
    expect(x.message).toBe("3 fichiers reçus");
    expect(pourcentage(x)).toBe(100);
  });

  it("un succès sans total connu garde ce qui a été compté", () => {
    const x = transfert(1, { etat: "en-cours", fait: 12, total: 0 });
    conclure(x, { ok: true, message: "" });
    expect(x.fait).toBe(12);
  });

  it("« Transfert annulé » du cœur classe la ligne en annulée, pas en erreur", () => {
    const x = transfert(1, { etat: "en-cours" });
    conclure(x, { ok: false, erreur: "Error: Transfert annulé (reprise possible)" });
    expect(x.etat).toBe("annule");
    expect(detailTransfert(x)).toBe(t("sftp-transfert-annule"));
  });

  it("toute autre erreur reste une erreur et la ligne montre son motif", () => {
    const x = transfert(1, { etat: "en-cours" });
    conclure(x, { ok: false, erreur: "Permission refusée" });
    expect(x.etat).toBe("erreur");
    expect(detailTransfert(x)).toBe("Permission refusée");
  });
});

describe("file des transferts : progression", () => {
  const p = (done: number, total: number, extra: Partial<{ fichier: string; termines: number; nombre: number }> = {}) =>
    ({ transfert: 1, fichier: "", done, total, termines: 0, nombre: 1, ...extra });

  it("la vitesse vaut la mesure instantanée à la première demi-seconde, puis se lisse", () => {
    const x = transfert(1, { etat: "en-cours", dernierT: 0 });
    appliquerProgression(x, p(1000, 10_000), 1000); // 1000 o en 1 s
    expect(x.vitesse).toBe(1000);
    appliquerProgression(x, p(4000, 10_000), 2000); // 3000 o/s mesurés
    expect(x.vitesse).toBeCloseTo(1000 * 0.6 + 3000 * 0.4);
  });

  it("la vitesse n'est pas recalculée sous la demi-seconde, l'avancement si", () => {
    const x = transfert(1, { etat: "en-cours", dernierT: 0 });
    appliquerProgression(x, p(1000, 10_000), 1000);
    appliquerProgression(x, p(1500, 10_000), 1200);
    expect(x.vitesse).toBe(1000);
    expect(x.fait).toBe(1500);
    expect(pourcentage(x)).toBe(15);
  });

  it("le fichier en cours n'est retenu que pour un transfert de plusieurs éléments", () => {
    const x = transfert(1, { etat: "en-cours" });
    appliquerProgression(x, p(1, 2, { fichier: "a.txt", nombre: 1 }), 0);
    expect(x.fichier).toBe("");
    appliquerProgression(x, p(1, 2, { fichier: "b.txt", nombre: 3, termines: 1 }), 0);
    expect(x.fichier).toBe("b.txt");
    expect(detailTransfert(x)).toContain("b.txt");
  });

  it("une ligne sans total connu reste à zéro pour cent", () => {
    expect(pourcentage({ etat: "en-cours", fait: 500, total: 0 })).toBe(0);
  });
});

describe("file des transferts : ce que la ligne affiche", () => {
  it("une ligne en attente montre des points de suspension", () => {
    expect(detailTransfert(transfert(1))).toBe("…");
  });

  it("un transfert d'un seul fichier montre « fait / total »", () => {
    const x = transfert(1, { etat: "en-cours", fait: 2048, total: 4096 });
    expect(detailTransfert(x)).toBe(`${humanSize(2048, langue())} / ${humanSize(4096, langue())}`);
  });

  it("une copie directe en cours dit ce qui se passe au lieu de « 0 o »", () => {
    const x = { ...transfert(1, { etat: "en-cours" }), kind: "copie" as const, direct: true };
    expect(detailTransfert(x)).toBe(t("sftp-copie-directe-en-cours"));
  });

  it("seule une ligne terminée s'efface d'un clic", () => {
    expect(effacableAuClic("fini")).toBe(true);
    expect(effacableAuClic("erreur")).toBe(true);
    expect(effacableAuClic("annule")).toBe(true);
    expect(effacableAuClic("attente")).toBe(false);
    expect(effacableAuClic("en-cours")).toBe(false);
  });

  it("le bouton « Annuler » et l'effacement au clic ne coexistent jamais", () => {
    const etats = ["attente", "en-cours", "fini", "erreur", "annule"] as const;
    for (const etat of etats) {
      for (const direct of [false, true]) {
        expect(boutonAnnulerVisible(etat, "copie", direct) && effacableAuClic(etat)).toBe(false);
      }
    }
  });
});
