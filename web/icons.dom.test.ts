// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { ic, hydrateIcons } from "./icons";

describe("ic", () => {
  it("produit un SVG avec la classe ic-svg", () => {
    expect(ic("plus")).toContain("<svg");
    expect(ic("plus")).toContain("ic-svg");
  });
  it("retombe sur l'icône fichier pour un nom inconnu", () => {
    expect(ic("nexistepas")).toBe(ic("file"));
  });
});

describe("hydrateIcons (DOM)", () => {
  it("remplace un bouton manual-btn par icône encadrée + label", () => {
    const root = document.createElement("div");
    root.innerHTML = `<button class="manual-btn" data-icon="key">Mes clés SSH</button>`;
    hydrateIcons(root);
    const btn = root.querySelector(".manual-btn")!;
    expect(btn.querySelector(".ic svg")).toBeTruthy();
    expect(btn.querySelector("span:last-child")!.textContent).toBe("Mes clés SSH");
    expect(btn.getElementsByTagName("svg").length).toBe(1);
  });
  it("bouton icône seul : juste le SVG", () => {
    const root = document.createElement("div");
    root.innerHTML = `<button data-icon="refresh" title="Rafraîchir"></button>`;
    hydrateIcons(root);
    const btn = root.querySelector("button")!;
    expect(btn.querySelector("svg")).toBeTruthy();
    expect(btn.textContent).toBe("");
  });
  it("un label sans manual-btn devient un span à côté du SVG", () => {
    const root = document.createElement("div");
    root.innerHTML = `<button class="btn-mini" data-icon="upload">Envoyer…</button>`;
    hydrateIcons(root);
    const btn = root.querySelector(".btn-mini")!;
    expect(btn.querySelector("svg")).toBeTruthy();
    expect(btn.querySelector("span")!.textContent).toBe("Envoyer…");
  });
});
