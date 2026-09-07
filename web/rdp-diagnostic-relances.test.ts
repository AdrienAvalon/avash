// @vitest-environment jsdom
// Trouvé par l'audit du 7 septembre 2026 : l'incrustation « Connexion RDP
// fermée » lisait `rdp_diagnostic` une seule fois, tout de suite. Or côté Rust
// `rdp_close` (émis juste avant, exécuté en ligne) retirait le journal, et la
// dernière ligne « Error: … » du sidecar est écrite APRÈS la fermeture de la
// WebSocket : la lecture unique rendait une chaîne vide et la raison de la
// coupure n'était jamais montrée. Ces tests verrouillent la lecture avec
// relances tant que la réponse reste vide.
import { describe, it, expect, beforeAll, vi } from "vitest";
import indexHtml from "./index.html?raw";

// rdp.ts câble des écouteurs et importe le cœur du front à l'évaluation ; on
// remplace ./main (effets de bord lourds) et on monte index.html pour que les
// autres modules trouvent leurs éléments, afin de n'exercer que
// `lireDiagnosticRdp`.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }) }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ setFullscreen: () => Promise.resolve() }) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: vi.fn(), writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./main", () => ({
  loadHosts: vi.fn(), renderHosts: vi.fn(), moveHostTo: vi.fn(), setupFolderDrop: vi.fn(),
  closeSession: vi.fn(), focusSession: vi.fn(), openSession: vi.fn(),
}));

/** Le corps de index.html, sans le script module (jsdom ne l'exécute pas). */
function corpsIndex(): string {
  const corps = indexHtml.slice(indexHtml.indexOf("<body>") + 6, indexHtml.indexOf("</body>"));
  return corps.replace(/<script[\s\S]*?<\/script>/g, "");
}

let lireDiagnosticRdp: (
  id: number,
  lire?: (id: number) => Promise<string>,
  pause?: (ms: number) => Promise<void>,
) => Promise<string>;

beforeAll(async () => {
  document.body.innerHTML = corpsIndex();
  ({ lireDiagnosticRdp } = await import("./rdp"));
});

/** Une pause instantanée : les tests ne doivent pas attendre 300 ms pour de vrai. */
const sansDelai = () => Promise.resolve();

describe("lireDiagnosticRdp", () => {
  it("relit tant que la réponse est vide et rend la première ligne non vide", async () => {
    // La ligne « Error: » du sidecar arrive après la fermeture de la socket :
    // les deux premières lectures sont vides, la troisième porte la raison.
    const lire = vi.fn<(id: number) => Promise<string>>()
      .mockResolvedValueOnce("")
      .mockResolvedValueOnce("   ")
      .mockResolvedValueOnce("Error: le serveur a fermé la connexion\n");
    const diag = await lireDiagnosticRdp(42, lire, sansDelai);
    expect(lire).toHaveBeenCalledTimes(3);
    expect(diag).toBe("Error: le serveur a fermé la connexion");
  });

  it("rend le diagnostic dès la première lecture non vide, sans relance inutile", async () => {
    const lire = vi.fn<(id: number) => Promise<string>>().mockResolvedValue("connecté : 10.0.0.1:3389");
    const diag = await lireDiagnosticRdp(7, lire, sansDelai);
    expect(lire).toHaveBeenCalledTimes(1);
    expect(diag).toBe("connecté : 10.0.0.1:3389");
  });

  it("abandonne après trois lectures vides plutôt que de boucler", async () => {
    const lire = vi.fn<(id: number) => Promise<string>>().mockResolvedValue("");
    const diag = await lireDiagnosticRdp(1, lire, sansDelai);
    expect(lire).toHaveBeenCalledTimes(3);
    expect(diag).toBe("");
  });

  it("traite une lecture en échec comme une réponse vide et poursuit les relances", async () => {
    const lire = vi.fn<(id: number) => Promise<string>>()
      .mockRejectedValueOnce(new Error("IPC coupé"))
      .mockResolvedValueOnce("lecture PDU : flux interrompu");
    const diag = await lireDiagnosticRdp(3, lire, sansDelai);
    expect(lire).toHaveBeenCalledTimes(2);
    expect(diag).toBe("lecture PDU : flux interrompu");
  });
});
