// Dossiers : création, menu contextuel, déplacement d'hôtes.

import { invoke } from "@tauri-apps/api/core";
import { allFolderPaths } from "./filters";
import { $, collapsedFolders, saveCollapsed, state } from "./etat";
import { askConfirm, askText } from "./dialogues";
import { closeAllContextMenus, placerMenu } from "./menu-hote";
import { loadHosts, moveHostTo, setupFolderDrop } from "./main";
import { notifyErreur } from "./notifications";
import { t } from "./i18n";

// ---------- Gestion des dossiers (création, menu, déplacement) ----------

/** Ensemble des dossiers connus (registre + dérivés des hôtes), triés. */
function allFolders(): string[] {
  return allFolderPaths(state.folders, [
    ...state.hosts.map((h) => h.folder ?? ""),
    ...state.rdpHosts.map((h) => h.folder ?? ""),
  ]);
}

async function createFolder(parent: string) {
  const name = await askText(
    parent ? t("dossiers-nouveau-sous-dossier") : t("nouveau-dossier-2"),
    parent ? t("dossiers-dans", { parent }) : t("dossiers-nom"),
    "",
  );
  if (!name || !name.trim()) return;
  const path = parent ? `${parent}/${name.trim()}` : name.trim();
  try {
    await invoke("folder_create", { path });
    collapsedFolders.delete(parent);
    saveCollapsed();
    await loadHosts();
  } catch (e) {
    notifyErreur(t("dossiers-creation-impossible", { e: String(e) }));
  }
}

$("new-folder-btn").addEventListener("click", () => void createFolder(""));

export function openFolderMenu(path: string, e: MouseEvent) {
  closeAllContextMenus();
  const m = $("folder-context");
  m.dataset.path = path;
  placerMenu(m, e);
  m.classList.add("open");
}
window.addEventListener("click", () => $("folder-context").classList.remove("open"));
$("folder-context").addEventListener("click", async (e) => {
  const act = (e.target as HTMLElement).closest("[data-act]")?.getAttribute("data-act");
  const path = $("folder-context").dataset.path ?? "";
  $("folder-context").classList.remove("open");
  if (!act || !path) return;
  if (act === "new") {
    await createFolder(path);
  } else if (act === "rename") {
    const leaf = path.split("/").pop() ?? path;
    const name = await askText(t("dossiers-renommer"), t("dossiers-nouveau-nom", { nom: leaf }), leaf);
    if (!name || !name.trim() || name.trim() === leaf) return;
    const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
    const to = parent ? `${parent}/${name.trim()}` : name.trim();
    try {
      await invoke("folder_rename", { from: path, to });
      await loadHosts();
    } catch (ex) {
      notifyErreur(t("dossiers-renommage-impossible", { e: String(ex) }));
    }
  } else if (act === "delete") {
    if (!(await askConfirm(t("dossiers-supprimer-question", { chemin: path })))) return;
    try {
      await invoke("folder_delete", { path });
      await loadHosts();
    } catch (ex) {
      notifyErreur(t("suppression-impossible", { e: String(ex) }));
    }
  }
});

let moveTarget: { kind: string; id: string } | null = null;
export function openMoveModal(kind: string, id: string) {
  moveTarget = { kind, id };
  $("move-error").hidden = true;
  ($("move-new") as HTMLInputElement).value = "";
  const listEl = $("move-list");
  listEl.innerHTML = "";
  const addRow = (label: string, folder: string) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "move-row";
    row.textContent = label;
    row.addEventListener("click", () => void doMove(folder));
    listEl.appendChild(row);
  };
  addRow(t("dossiers-racine"), "");
  for (const f of allFolders()) addRow(f, f);
  $("move-modal").classList.add("open");
  setTimeout(() => ($("move-new") as HTMLInputElement).focus(), 30);
}
export function closeMoveModal() {
  $("move-modal").classList.remove("open");
  moveTarget = null;
}
async function doMove(folder: string) {
  if (!moveTarget) return;
  await moveHostTo(moveTarget.kind, moveTarget.id, folder);
  closeMoveModal();
}
$("move-cancel").addEventListener("click", closeMoveModal);
$("move-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = ($("move-new") as HTMLInputElement).value.trim();
  if (!f) {
    $("move-error").textContent = t("dossiers-saisis-nom");
    $("move-error").hidden = false;
    return;
  }
  void doMove(f);
});

// Racine : d\u00e9poser un h\u00f4te sur la zone vide de la liste le remet \u00e0 la racine.
setupFolderDrop($("host-list"), "", false);
