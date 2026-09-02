// Import de sessions depuis PuTTY et MobaXterm.
//
// Le cœur lit et propose ; ici on montre, on laisse cocher et renommer, puis
// on écrit d'un coup. Un hôte déjà déclaré qui vise le même serveur est
// proposé décoché, avec le nom sous lequel il existe.

import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { type Host } from "./filters";
import { $ } from "./etat";
import { notify, notifyErreur } from "./notifications";
import { loadHosts } from "./main";

type Candidat = {
  source: "putty" | "mobaxterm";
  nom_origine: string;
  host: Host & { folder: string; identity_file?: string | null };
  remarques: string[];
  doublon: string | null;
};
type Bilan = { candidats: Candidat[]; ignorees: number; consultes: string[] };

let candidats: Candidat[] = [];

const modal = () => $("import-modal");
const liste = () => $("import-list");

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

function rendre(bilan: Bilan) {
  candidats = bilan.candidats;
  const consultes = $("import-consultes");
  consultes.textContent = bilan.consultes.length
    ? `Consulté : ${bilan.consultes.join(" · ")}`
    : "Aucun emplacement habituel trouvé sur cette machine : désignez un dossier de sessions PuTTY ou un fichier MobaXterm.ini.";
  const l = liste();
  l.innerHTML = "";
  if (candidats.length === 0) {
    l.innerHTML = `<div class="empty">Aucune session SSH à importer.</div>`;
  }
  candidats.forEach((c, i) => {
    const row = document.createElement("div");
    row.className = "import-row" + (c.doublon ? " doublon" : "");
    row.dataset.alias = c.host.alias;
    const port = c.host.port ?? 22;
    const cible = `${c.host.user ? c.host.user + "@" : ""}${c.host.hostname ?? "?"}:${port}`;
    const dossier = c.host.folder ? ` · dossier ${escapeHtml(c.host.folder)}` : "";
    const cle = c.host.identity_file ? ` · clé ${escapeHtml(c.host.identity_file)}` : "";
    const doublon = c.doublon ? `<div class="import-remarque">Déjà déclaré sous « ${escapeHtml(c.doublon)} » : décoché pour ne pas dupliquer.</div>` : "";
    const remarques = c.remarques.map((r) => `<div class="import-remarque">${escapeHtml(r)}</div>`).join("");
    row.innerHTML = `
      <input type="checkbox" id="import-c-${i}" ${c.doublon ? "" : "checked"} aria-label="Importer ${escapeHtml(c.nom_origine)}" />
      <label class="import-alias" for="import-c-${i}"><input type="text" value="${escapeHtml(c.host.alias)}" aria-label="Alias" spellcheck="false" /><span class="import-source">${c.source === "putty" ? "PuTTY" : "MobaXterm"}</span></label>
      <div class="import-detail">${escapeHtml(c.nom_origine)} → ${escapeHtml(cible)}${dossier}${cle}</div>
      ${doublon}${remarques}`;
    l.appendChild(row);
  });
  const bilanEl = $("import-bilan");
  const parties: string[] = [];
  if (bilan.ignorees > 0) parties.push(`${bilan.ignorees} session${bilan.ignorees > 1 ? "s" : ""} d'un autre protocole (RDP, telnet, série) laissée${bilan.ignorees > 1 ? "s" : ""} de côté.`);
  bilanEl.textContent = parties.join(" ");
  majBouton();
}

function selection(): { host: Candidat["host"] }[] {
  return [...liste().querySelectorAll<HTMLElement>(".import-row")].flatMap((row, i) => {
    const coche = row.querySelector<HTMLInputElement>("input[type=checkbox]")!.checked;
    if (!coche) return [];
    const alias = row.querySelector<HTMLInputElement>("input[type=text]")!.value.trim();
    return [{ host: { ...candidats[i].host, alias } }];
  });
}

function majBouton() {
  ($("import-submit") as HTMLButtonElement).disabled = selection().length === 0;
}

async function analyser(chemin?: string) {
  liste().innerHTML = `<div class="empty">Lecture…</div>`;
  try {
    rendre(await invoke<Bilan>("import_scan", { chemin: chemin ?? null }));
  } catch (e) {
    liste().innerHTML = "";
    $("import-bilan").textContent = "";
    notifyErreur(`Import impossible : ${e}`);
  }
}

async function importOpen() {
  modal().classList.add("open");
  await analyser();
}

function importClose() {
  modal().classList.remove("open");
}

async function importChoisir() {
  let choisi: string | string[] | null;
  try {
    // Un fichier (MobaXterm.ini, .mxtsessions) ou un dossier (sessions PuTTY) :
    // le sélecteur natif ne propose pas les deux à la fois, on demande d'abord
    // un fichier, et l'annulation bascule sur un dossier.
    choisi = await openDialog({ multiple: false, directory: false, title: "MobaXterm.ini ou .mxtsessions" });
    if (!choisi) choisi = await openDialog({ multiple: false, directory: true, title: "Dossier des sessions PuTTY" });
  } catch (e) {
    notifyErreur(`Sélecteur indisponible : ${e}`);
    return;
  }
  if (!choisi) return;
  await analyser(Array.isArray(choisi) ? choisi[0] : choisi);
}

async function importSubmit() {
  const hosts = selection().map((s) => s.host);
  if (hosts.length === 0) return;
  const btn = $("import-submit") as HTMLButtonElement;
  btn.disabled = true;
  try {
    const n = await invoke<number>("import_apply", { hosts });
    importClose();
    notify(`${n} hôte${n > 1 ? "s" : ""} importé${n > 1 ? "s" : ""} dans ~/.ssh/config.`, "succes");
    await loadHosts();
  } catch (e) {
    notifyErreur(`Import interrompu : ${e}`);
  } finally {
    btn.disabled = false;
  }
}

$("import-btn").addEventListener("click", () => void importOpen());
$("import-cancel").addEventListener("click", importClose);
$("import-choisir").addEventListener("click", () => void importChoisir());
$("import-submit").addEventListener("click", () => void importSubmit());
liste().addEventListener("change", majBouton);
liste().addEventListener("input", majBouton);
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && modal().classList.contains("open")) importClose();
});
