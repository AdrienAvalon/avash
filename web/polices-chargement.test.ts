// @vitest-environment jsdom
// Audit du 12 septembre 2026 (C-front-8) : `ensureFontLoaded`, appelé au
// démarrage, chargeait les deux graisses de la police embarquée (975 Ko), dont
// la grasse que seul le terminal utilise. Au lancement à froid, l'accueil
// payait la lecture et le décodage d'un mégaoctet. La grasse attend désormais
// le premier terminal, qui l'attend avant de mesurer ses caractères.
import { describe, it, expect, beforeAll, vi } from "vitest";

vi.mock("./main", () => ({ renderHosts: vi.fn() }));

const charges: string[] = [];

beforeAll(() => {
  window.matchMedia = ((): MediaQueryList =>
    ({ matches: false, addEventListener: () => {}, removeEventListener: () => {} }) as unknown as MediaQueryList) as typeof window.matchMedia;
  Object.defineProperty(document, "fonts", {
    configurable: true,
    value: { load: (f: string) => { charges.push(f); return Promise.resolve([]); }, ready: Promise.resolve() },
  });
});

describe("chargement des polices", () => {
  it("l_accueil_ne_charge_que_la_graisse_reguliere", async () => {
    const { ensureFontLoaded } = await import("./theme");
    await ensureFontLoaded();
    expect(charges.some((f) => f.startsWith("400"))).toBe(true);
    expect(charges.some((f) => f.startsWith("600"))).toBe(false);
  });

  it("la_graisse_grasse_vient_avec_le_premier_terminal", async () => {
    const { ensureFontLoaded, ensureGrasseChargee } = await import("./theme");
    await ensureFontLoaded();
    await ensureGrasseChargee();
    await ensureGrasseChargee();
    expect(charges.filter((f) => f.startsWith("600"))).toHaveLength(1);
  });
});
