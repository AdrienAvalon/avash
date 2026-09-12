// @vitest-environment jsdom
// Audit du 12 septembre 2026 (couverture.md, section 5, point 2) : la décision
// d'avertissement sur les droits d'une clé privée n'avait aucun test, alors que
// le cœur verrouille exprès le `null` (Windows, droits portés par une ACL) pour
// elle. En l'écrivant, le test a montré une fausse alerte : tout mode différent
// de « 600 » était signalé, « 400 » compris, alors qu'OpenSSH ne refuse une clé
// que si le groupe ou les autres y ont accès (`st_mode & 077`, message
// « UNPROTECTED PRIVATE KEY FILE »). La décision suit désormais cette règle.
import { describe, it, expect, beforeAll, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

let droitsCle: (mode: string | null) => "acl" | "correct" | "trop-ouverts";
let keysOpen: () => Promise<void>;

beforeAll(async () => {
  // Juste ce que cles.ts câble à l'import et ce que keysRefresh remplit.
  document.body.innerHTML = `
    <button id="keys-btn"></button>
    <div id="keys-modal">
      <div id="key-list"></div>
      <div id="k-ok" hidden></div>
      <div id="k-error" hidden></div>
      <button id="k-close"></button>
      <form id="keygen-form"><button id="k-gen-submit"></button></form>
      <details id="deploy-block">
        <form id="deploy-form"><select id="d-key"></select><button id="d-submit"></button></form>
      </details>
    </div>`;
  const mod = await import("./cles");
  droitsCle = mod.droitsCle;
  keysOpen = mod.keysOpen;
  (await import("./i18n")).setLangue("fr");
});

describe("droitsCle : avertir seulement quand OpenSSH refuserait la clé", () => {
  it("600 est correct", () => {
    expect(droitsCle("600")).toBe("correct");
  });

  it("400 et 700 sont corrects : seuls les droits du groupe et des autres comptent", () => {
    expect(droitsCle("400")).toBe("correct");
    expect(droitsCle("700")).toBe("correct");
  });

  it("un droit ouvert au groupe ou aux autres avertit", () => {
    for (const mode of ["644", "640", "604", "660", "777", "601"]) expect(droitsCle(mode), mode).toBe("trop-ouverts");
  });

  it("null (Windows : droits portés par une ACL) n'avertit pas", () => {
    expect(droitsCle(null)).toBe("acl");
  });

  it("un mode illisible avertit plutôt que de se taire", () => {
    expect(droitsCle("")).toBe("trop-ouverts");
    expect(droitsCle("rw-------")).toBe("trop-ouverts");
  });
});

describe("liste des clés : l'avertissement suit la décision", () => {
  it("seules les clés aux droits trop ouverts portent le signe ⚠", async () => {
    invoke.mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "keys_list"
          ? [
              { name: "id_ok", path: "/k/id_ok", public_line: null, mode: "600" },
              { name: "id_lecture", path: "/k/id_lecture", public_line: null, mode: "400" },
              { name: "id_ouverte", path: "/k/id_ouverte", public_line: null, mode: "644" },
              { name: "id_windows", path: "C:\\k\\id_windows", public_line: null, mode: null },
            ]
          : [],
      ),
    );
    await keysOpen();
    const modes = new Map(
      [...document.querySelectorAll<HTMLElement>("#key-list .key-row")].map((r) => [
        r.querySelector(".kname")!.textContent,
        r.querySelector<HTMLElement>(".kmode")!,
      ]),
    );
    expect(modes.get("id_ok")!.classList.contains("warn")).toBe(false);
    expect(modes.get("id_ok")!.textContent).toBe("600");
    expect(modes.get("id_lecture")!.classList.contains("warn")).toBe(false);
    expect(modes.get("id_lecture")!.textContent).toBe("400");
    expect(modes.get("id_ouverte")!.classList.contains("warn")).toBe(true);
    expect(modes.get("id_ouverte")!.textContent).toBe("644 ⚠");
    expect(modes.get("id_windows")!.classList.contains("warn")).toBe(false);
    expect(modes.get("id_windows")!.textContent).toBe("ACL");
  });
});
