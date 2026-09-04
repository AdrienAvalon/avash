import { describe, expect, it } from "vitest";
import { listeAMemoriser, nombreARestaurer } from "./onglets-memoire";

describe("mémoire des onglets", () => {
  const alias = new Set(["web-1", "db-1"]);
  const bureaux = new Set(["b7"]);

  it("ne retient que les hôtes déclarés et les bureaux enregistrés, dans l'ordre, doublons compris", () => {
    expect(
      listeAMemoriser(
        [
          { kind: "ssh", alias: "web-1" },
          { kind: "ssh", alias: "deploy@203.0.113.9" },
          { kind: "rdp", hostId: "b7" },
          { kind: "rdp" },
          { kind: "ssh", alias: "web-1" },
        ],
        alias,
        bureaux,
      ),
    ).toEqual([
      { kind: "ssh", alias: "web-1" },
      { kind: "rdp", host_id: "b7" },
      { kind: "ssh", alias: "web-1" },
    ]);
  });

  it("compte ce qui existe encore : un hôte supprimé depuis n'est pas proposé", () => {
    expect(
      nombreARestaurer(
        [
          { kind: "ssh", alias: "web-1" },
          { kind: "ssh", alias: "disparu" },
          { kind: "rdp", host_id: "b7" },
          { kind: "rdp", host_id: "b9" },
        ],
        alias,
        bureaux,
      ),
    ).toBe(2);
    expect(nombreARestaurer([], alias, bureaux)).toBe(0);
  });
});
