// Liste des enregistrements de session, et le dossier qui les contient.
//
// Un toast donnait le chemin à l'arrêt, puis plus rien : la liste se retrouve
// ici, du plus récent au plus ancien, avec de quoi ouvrir le dossier dans le
// gestionnaire de fichiers ou copier un chemin pour `asciinema play`.

import { invoke } from "@tauri-apps/api/core";
import { $ } from "./etat";
import { humanSize, shortDate, stripHtml } from "./filters";
import { langue, t } from "./i18n";
import { notify, notifyErreur } from "./notifications";

type Info = { chemin: string; nom: string; octets: number; modifie: number };

const modal = () => $("enregistrements-modal");

function rendre(liste: Info[]) {
  const zone = $("enregistrements-liste");
  zone.innerHTML = "";
  if (liste.length === 0) {
    zone.innerHTML = `<div class="empty">${stripHtml(t("enregistrements-aucun"))}</div>`;
    return;
  }
  for (const e of liste) {
    const row = document.createElement("div");
    row.className = "enregistrement-row";
    row.dataset.chemin = e.chemin;
    row.innerHTML = `<span class="nom"></span><span class="meta"></span><button type="button" class="btn-ghost" data-act="copier"></button>`;
    row.querySelector(".nom")!.textContent = e.nom;
    row.querySelector(".meta")!.textContent = `${humanSize(e.octets, langue())} · ${shortDate(e.modifie, new Date(), langue())}`;
    const btn = row.querySelector("button")!;
    btn.textContent = t("copier-le-chemin");
    btn.addEventListener("click", () => {
      navigator.clipboard.writeText(e.chemin).then(
        () => notify(t("sftp-chemin-copie", { chemin: e.chemin }), "succes"),
        () => notifyErreur(t("enregistrements-copie-impossible")),
      );
    });
    zone.appendChild(row);
  }
}

export async function enregistrementsOpen() {
  modal().classList.add("open");
  try {
    rendre(await invoke<Info[]>("enregistrements_lister"));
  } catch (e) {
    notifyErreur(t("enregistrements-lecture-impossible", { e: String(e) }));
  }
}

function enregistrementsClose() {
  modal().classList.remove("open");
}

$("enregistrements-close").addEventListener("click", enregistrementsClose);
$("enregistrements-dossier").addEventListener("click", () => {
  invoke<string>("enregistrements_ouvrir_dossier").catch((e) => notifyErreur(t("enregistrements-dossier-impossible", { e: String(e) })));
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && modal().classList.contains("open")) enregistrementsClose();
});
