// Dossiers : création, menu contextuel, déplacement d'hôtes.

import { invoke } from "@tauri-apps/api/core";
import { allFolderPaths } from "./filters";
import { $, collapsedFolders, saveCollapsed, state } from "./etat";
import { askConfirm, askText } from "./dialogues";
import { closeAllContextMenus, placerMenu } from "./menu-hote";
import { loadHosts, moveHostTo, setupFolderDrop } from "./main";
import { notifyErreur } from "./notifications";

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
    parent ? "Nouveau sous-dossier" : "Nouveau dossier",
    parent ? `Dans \u00ab ${parent} \u00bb` : "Nom du dossier",
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
    notifyErreur(`Cr\u00e9ation impossible : ${e}`);
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
    const name = await askText("Renommer le dossier", `Nouveau nom de \u00ab ${leaf} \u00bb`, leaf);
    if (!name || !name.trim() || name.trim() === leaf) return;
    const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
    const to = parent ? `${parent}/${name.trim()}` : name.trim();
    try {
      await invoke("folder_rename", { from: path, to });
      await loadHosts();
    } catch (ex) {
      notifyErreur(`Renommage impossible : ${ex}`);
    }
  } else if (act === "delete") {
    if (!(await askConfirm(`Supprimer le dossier \u00ab ${path} \u00bb ?\n\nLes h\u00f4tes qu'il contient (et ses sous-dossiers) reviennent \u00e0 la racine ; ils ne sont pas supprim\u00e9s.`))) return;
    try {
      await invoke("folder_delete", { path });
      await loadHosts();
    } catch (ex) {
      notifyErreur(`Suppression impossible : ${ex}`);
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
  addRow("\u2196 Racine", "");
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
    $("move-error").textContent = "Saisis un nom de dossier.";
    $("move-error").hidden = false;
    return;
  }
  void doMove(f);
});

// Racine : d\u00e9poser un h\u00f4te sur la zone vide de la liste le remet \u00e0 la racine.
setupFolderDrop($("host-list"), "", false);
