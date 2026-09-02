// Snippets : liste, formulaire, flux d'envoi (variables puis cibles).

import { invoke } from "@tauri-apps/api/core";
import { ic } from "./icons";
import { snippetPreview, snippetVars, renderSnippet, type Snippet } from "./filters";
import { $, state } from "./etat";
import { askConfirm } from "./dialogues";
import { focusSession } from "./main";
import { notifyErreur } from "./notifications";

// ---------- Snippets ----------

type OpenSession = { id: number; label: string };

const snip = {
  list: [] as Snippet[],
  focusId: null as string | null,
};
export const snippetsModal = () => $("snippets-modal");

async function snippetsRefresh() {
  try {
    snip.list = await invoke<Snippet[]>("snippet_list");
  } catch (e) {
    $("sn-error").textContent = String(e);
    $("sn-error").hidden = false;
    return;
  }
  renderSnippets();
}

function renderSnippets() {
  const list = $("snippet-list");
  list.innerHTML = "";
  if (snip.list.length === 0) {
    const empty = document.createElement("div");
    empty.className = "snippet-empty";
    empty.textContent = "Aucun snippet. Crée-en un ci-dessous.";
    list.appendChild(empty);
    return;
  }
  for (const sn of [...snip.list].sort((a, b) => a.name.localeCompare(b.name))) {
    const nVars = snippetVars(sn.command).length;
    const row = document.createElement("div");
    row.className = "snippet-row";
    row.innerHTML = `<div class="smain">
        <div class="sname"><span class="snm"></span></div>
        <div class="scmd"></div>
      </div>
      <div class="sacts">
        <button class="tbtn go" data-act="send" title="Envoyer">${ic("play")}</button>
        <button class="tbtn" data-act="edit" title="Modifier">${ic("pencil")}</button>
        <button class="tbtn danger" data-act="delete" title="Supprimer">${ic("trash")}</button>
      </div>`;
    row.querySelector(".snm")!.textContent = sn.name;
    if (nVars > 0) {
      const b = document.createElement("span");
      b.className = "svar";
      b.textContent = `${nVars} var${nVars > 1 ? "s" : ""}`;
      row.querySelector(".sname")!.appendChild(b);
    }
    row.querySelector(".scmd")!.textContent = snippetPreview(sn.command);
    row.querySelector('[data-act="send"]')!.addEventListener("click", () => snippetSendFlow(sn));
    row.querySelector('[data-act="edit"]')!.addEventListener("click", () => snippetEdit(sn));
    row.querySelector('[data-act="delete"]')!.addEventListener("click", () => snippetDelete(sn));
    list.appendChild(row);
  }
}

// ----- Formulaire -----

function snippetFormReset() {
  ($("snippet-form") as HTMLFormElement).reset();
  ($("sn-id") as HTMLInputElement).value = "";
  ($("sn-run") as HTMLInputElement).checked = true;
  $("snippet-form-title").textContent = "Nouveau snippet";
  $("sn-submit").textContent = "Enregistrer";
  $("sn-reset").hidden = true;
  $("sn-error").hidden = true;
  snippetSyncVars();
}

function snippetSyncVars() {
  const vars = snippetVars(($("sn-command") as HTMLTextAreaElement).value);
  $("sn-vars").textContent = vars.length ? `Variables : ${vars.map((v) => `{{${v}}}`).join(", ")}` : "";
}

function snippetEdit(sn: Snippet) {
  ($("sn-id") as HTMLInputElement).value = sn.id;
  ($("sn-name") as HTMLInputElement).value = sn.name;
  ($("sn-command") as HTMLTextAreaElement).value = sn.command;
  ($("sn-run") as HTMLInputElement).checked = sn.run;
  $("snippet-form-title").textContent = `Modifier « ${sn.name} »`;
  $("sn-submit").textContent = "Enregistrer les modifications";
  $("sn-reset").hidden = false;
  ($("snippet-block") as HTMLDetailsElement).open = true;
  snippetSyncVars();
  ($("sn-name") as HTMLInputElement).focus();
}

async function snippetDelete(sn: Snippet) {
  if (!(await askConfirm(`Supprimer le snippet « ${sn.name} » ?`))) return;
  try {
    await invoke("snippet_delete", { id: sn.id });
    await snippetsRefresh();
  } catch (e) {
    notifyErreur(`Suppression impossible : ${e}`);
  }
}

async function snippetsOpen() {
  snippetFormReset();
  snippetsModal().classList.add("open");
  await snippetsRefresh();
  ($("snippet-block") as HTMLDetailsElement).open = snip.list.length === 0;
}
export function snippetsClose() { snippetsModal().classList.remove("open"); }

$("snippets-btn").addEventListener("click", snippetsOpen);
$("sn-close").addEventListener("click", snippetsClose);
$("sn-reset").addEventListener("click", snippetFormReset);
$("sn-command").addEventListener("input", snippetSyncVars);

$("snippet-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const err = $("sn-error");
  const submit = $("sn-submit") as HTMLButtonElement;
  submit.disabled = true;
  try {
    await invoke("snippet_save", {
      id: ($("sn-id") as HTMLInputElement).value || null,
      name: ($("sn-name") as HTMLInputElement).value.trim(),
      command: ($("sn-command") as HTMLTextAreaElement).value,
      run: ($("sn-run") as HTMLInputElement).checked,
      category: null,
    });
    snippetFormReset();
    ($("snippet-block") as HTMLDetailsElement).open = false;
    await snippetsRefresh();
  } catch (ex) {
    err.textContent = String(ex);
    err.hidden = false;
  } finally {
    submit.disabled = false;
  }
});

// ----- Flux d'envoi : variables puis cibles -----

let sendCtx: { snippet: Snippet; sessions: OpenSession[] } | null = null;

async function snippetSendFlow(sn: Snippet) {
  let sessions: OpenSession[];
  try {
    sessions = await invoke<OpenSession[]>("open_sessions");
  } catch {
    sessions = [];
  }
  if (sessions.length === 0) {
    $("sn-error").textContent = "Ouvre d'abord une session : un snippet s'envoie dans un terminal.";
    $("sn-error").hidden = false;
    return;
  }
  sendCtx = { snippet: sn, sessions };
  $("send-title").textContent = sn.name;
  ($("send-run") as HTMLInputElement).checked = sn.run;

  // Champs pour chaque variable.
  const varsBox = $("send-vars");
  varsBox.innerHTML = "";
  for (const v of snippetVars(sn.command)) {
    // Le nom de variable vient du snippet : on le pose via le DOM (dataset,
    // textContent) et jamais via innerHTML, pour qu'un « " » ou « > » dans un
    // nom ne casse pas l'attribut.
    const label = document.createElement("label");
    const span = document.createElement("span");
    span.textContent = v;
    const input = document.createElement("input");
    input.spellcheck = false;
    input.dataset.var = v;
    label.append(span, input);
    varsBox.appendChild(label);
  }

  // Cibles : cases a cocher si plusieurs sessions ; l'active pre-cochee.
  const wrap = $("send-targets-wrap");
  const targets = $("send-targets");
  targets.innerHTML = "";
  if (sessions.length > 1) {
    wrap.hidden = false;
    for (const se of sessions) {
      const label = document.createElement("label");
      const checked = se.id === state.active ? "checked" : "";
      label.innerHTML = `<input type="checkbox" data-sid="${se.id}" ${checked}/><span></span>`;
      label.querySelector("span")!.textContent = se.label;
      targets.appendChild(label);
    }
  } else {
    wrap.hidden = true;
  }

  updateSendPreview();
  $("send-error").hidden = true;
  $("send-modal").classList.add("open");
  setTimeout(() => (varsBox.querySelector("input") as HTMLInputElement | null)?.focus(), 30);
}

function currentVars(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const i of $("send-vars").querySelectorAll<HTMLInputElement>("input[data-var]")) {
    out[i.dataset.var!] = i.value;
  }
  return out;
}

function updateSendPreview() {
  if (!sendCtx) return;
  $("send-preview").textContent = renderSnippet(sendCtx.snippet.command, currentVars());
}

$("send-vars").addEventListener("input", updateSendPreview);
$("send-cancel").addEventListener("click", () => $("send-modal").classList.remove("open"));
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && $("send-modal").classList.contains("open")) $("send-modal").classList.remove("open");
});

$("send-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (!sendCtx) return;
  const command = renderSnippet(sendCtx.snippet.command, currentVars());
  const run = ($("send-run") as HTMLInputElement).checked;
  let ids: number[];
  if (sendCtx.sessions.length > 1) {
    ids = [...$("send-targets").querySelectorAll<HTMLInputElement>("input:checked")].map((i) => Number(i.dataset.sid));
    if (ids.length === 0) {
      $("send-error").textContent = "Choisis au moins une session.";
      $("send-error").hidden = false;
      return;
    }
  } else {
    ids = [sendCtx.sessions[0].id];
  }
  try {
    const n = await invoke<number>("snippet_send", { sessionIds: ids, command, run });
    $("send-modal").classList.remove("open");
    snippetsClose();
    // Retour a l'onglet vise (le premier), pour voir le resultat.
    focusSession(ids[0]);
    void n;
  } catch (ex) {
    $("send-error").textContent = String(ex);
    $("send-error").hidden = false;
  }
});
