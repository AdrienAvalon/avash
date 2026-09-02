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
import { t } from "./i18n";

type Candidat = {
  source: "putty" | "mobaxterm";
  nom_origine: string;
  host: Host & { folder: string; identity_file?: string | null };
  ppk: string | null;
  remarques: string[];
  doublon: string | null;
};
type Bureau = {
  source: "putty" | "mobaxterm";
  nom_origine: string;
  name: string;
  host: string;
  port: number;
  user: string;
  folder: string;
  doublon: string | null;
};
type Bilan = { candidats: Candidat[]; bureaux: Bureau[]; ignorees: number; consultes: string[] };
type BilanApply = { hotes: number; bureaux: number; cles_converties: number; avertissements: string[] };

let candidats: Candidat[] = [];
let bureaux: Bureau[] = [];

const modal = () => $("import-modal");
const liste = () => $("import-list");

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

function rendre(bilan: Bilan) {
  candidats = bilan.candidats;
  bureaux = bilan.bureaux;
  const consultes = $("import-consultes");
  consultes.textContent = bilan.consultes.length
    ? t("import-consulte", { liste: bilan.consultes.join(" · ") })
    : t("import-aucun-emplacement");
  const l = liste();
  l.innerHTML = "";
  if (candidats.length === 0 && bureaux.length === 0) {
    l.innerHTML = `<div class="empty">${t("import-aucune-session")}</div>`;
  }
  candidats.forEach((c, i) => {
    const row = document.createElement("div");
    row.className = "import-row" + (c.doublon ? " doublon" : "");
    row.dataset.alias = c.host.alias;
    const port = c.host.port ?? 22;
    const cible = `${c.host.user ? c.host.user + "@" : ""}${c.host.hostname ?? "?"}:${port}`;
    const dossier = c.host.folder ? ` · ${t("import-dossier")} ${escapeHtml(c.host.folder)}` : "";
    const cle = c.host.identity_file ? ` · ${t("import-cle")} ${escapeHtml(c.host.identity_file)}` : "";
    const doublon = c.doublon ? `<div class="import-remarque">${escapeHtml(t("import-doublon", { alias: c.doublon }))}</div>` : "";
    const remarques = c.remarques.map((r) => `<div class="import-remarque">${escapeHtml(r)}</div>`).join("");
    row.innerHTML = `
      <input type="checkbox" id="import-c-${i}" ${c.doublon ? "" : "checked"} aria-label="${escapeHtml(t("importer"))} ${escapeHtml(c.nom_origine)}" />
      <label class="import-alias" for="import-c-${i}"><input type="text" value="${escapeHtml(c.host.alias)}" aria-label="Alias" spellcheck="false" /><span class="import-source">${c.source === "putty" ? "PuTTY" : "MobaXterm"}</span></label>
      <div class="import-detail">${escapeHtml(c.nom_origine)} → ${escapeHtml(cible)}${dossier}${cle}</div>
      ${doublon}${remarques}`;
    l.appendChild(row);
  });
  bureaux.forEach((b, i) => {
    const row = document.createElement("div");
    row.className = "import-row" + (b.doublon ? " doublon" : "");
    row.dataset.bureau = b.name;
    const cible = `${b.user ? b.user + "@" : ""}${b.host}:${b.port}`;
    const dossier = b.folder ? ` · ${t("import-dossier")} ${escapeHtml(b.folder)}` : "";
    const doublon = b.doublon ? `<div class="import-remarque">${escapeHtml(t("import-doublon", { alias: b.doublon }))}</div>` : "";
    row.innerHTML = `
      <input type="checkbox" id="import-b-${i}" ${b.doublon ? "" : "checked"} aria-label="${escapeHtml(t("importer"))} ${escapeHtml(b.nom_origine)}" />
      <label class="import-alias" for="import-b-${i}"><input type="text" value="${escapeHtml(b.name)}" aria-label="${escapeHtml(t("nom"))}" spellcheck="false" /><span class="import-source">RDP · ${b.source === "putty" ? "PuTTY" : "MobaXterm"}</span></label>
      <div class="import-detail">${escapeHtml(b.nom_origine)} → ${escapeHtml(cible)}${dossier}</div>
      ${doublon}`;
    l.appendChild(row);
  });
  const bilanEl = $("import-bilan");
  const parties: string[] = [];
  if (bilan.ignorees > 0) parties.push(t(bilan.ignorees > 1 ? "import-ignorees" : "import-ignoree", { n: bilan.ignorees }));
  bilanEl.textContent = parties.join(" ");
  majBouton();
}

function selection(): { hosts: { host: Candidat["host"]; ppk: string | null }[]; bureaux: Omit<Bureau, "doublon">[] } {
  const rows = [...liste().querySelectorAll<HTMLElement>(".import-row")];
  const coche = (row: HTMLElement) => row.querySelector<HTMLInputElement>("input[type=checkbox]")!.checked;
  const saisie = (row: HTMLElement) => row.querySelector<HTMLInputElement>("input[type=text]")!.value.trim();
  const hosts = rows.slice(0, candidats.length).flatMap((row, i) =>
    coche(row) ? [{ host: { ...candidats[i].host, alias: saisie(row) }, ppk: candidats[i].ppk }] : []);
  const retenus = rows.slice(candidats.length).flatMap((row, i) => {
    if (!coche(row)) return [];
    const { source, nom_origine, name, host, port, user, folder } = bureaux[i];
    return [{ source, nom_origine, name: saisie(row) || name, host, port, user, folder }];
  });
  return { hosts, bureaux: retenus };
}

function majBouton() {
  const sel = selection();
  ($("import-submit") as HTMLButtonElement).disabled = sel.hosts.length + sel.bureaux.length === 0;
}

async function analyser(chemin?: string) {
  liste().innerHTML = `<div class="empty">${t("import-lecture")}</div>`;
  try {
    rendre(await invoke<Bilan>("import_scan", { chemin: chemin ?? null }));
  } catch (e) {
    liste().innerHTML = "";
    $("import-bilan").textContent = "";
    notifyErreur(t("import-impossible", { e: String(e) }));
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
    choisi = await openDialog({ multiple: false, directory: false, title: t("import-titre-fichier") });
    if (!choisi) choisi = await openDialog({ multiple: false, directory: true, title: t("import-titre-dossier") });
  } catch (e) {
    notifyErreur(t("selecteur-indisponible", { e: String(e) }));
    return;
  }
  if (!choisi) return;
  await analyser(Array.isArray(choisi) ? choisi[0] : choisi);
}

async function importSubmit() {
  const sel = selection();
  if (sel.hosts.length + sel.bureaux.length === 0) return;
  const btn = $("import-submit") as HTMLButtonElement;
  btn.disabled = true;
  try {
    const bilan = await invoke<BilanApply>("import_apply", { hosts: sel.hosts, bureaux: sel.bureaux });
    importClose();
    const parties = [t(bilan.hotes > 1 ? "import-hotes-importes" : "import-hote-importe", { n: bilan.hotes })];
    if (bilan.bureaux > 0) parties.push(t(bilan.bureaux > 1 ? "import-bureaux-importes" : "import-bureau-importe", { n: bilan.bureaux }));
    if (bilan.cles_converties > 0) parties.push(t("import-cles-converties", { n: bilan.cles_converties }));
    notify(parties.join(" "), bilan.avertissements.length ? "info" : "succes");
    for (const a of bilan.avertissements) notifyErreur(a);
    await loadHosts();
  } catch (e) {
    notifyErreur(t("import-interrompu", { e: String(e) }));
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
