// Tunnels SSH : liste, formulaire, rafraîchissement.

import { invoke } from "@tauri-apps/api/core";
import { ic } from "./icons";
import { isPasswordRequired, describeTunnel, tunnelFlag, tunnelTraffic, activeTunnelsByHost, type TunnelDef, type TunnelStatus, type TunnelKind } from "./filters";
import { $, state } from "./etat";
import { askConfirm, askPassword } from "./dialogues";
import { notifyErreur } from "./notifications";
import { renderHosts } from "./main";
import { t } from "./i18n";

// ---------- Tunnels SSH ----------

export const tunnels = {
  defs: [] as TunnelDef[],
  status: new Map<string, TunnelStatus>(),
  /** Alias -> nombre de tunnels vivants (badge de la barre laterale). */
  byHost: new Map<string, number>(),
  /** Alias preselectionne quand la modale s'ouvre depuis un hote. */
  focusAlias: null as string | null,
  timer: null as number | null,
  /** Tunnels en cours de demarrage : evite le double clic. */
  busy: new Set<string>(),
};

const tunnelsModal = () => $("tunnels-modal");

/** Recharge definitions + etats, puis redessine liste et badges. */
async function tunnelsRefresh() {
  try {
    const [defs, status] = await Promise.all([
      invoke<TunnelDef[]>("tunnel_defs"),
      invoke<TunnelStatus[]>("tunnel_status"),
    ]);
    tunnels.defs = defs;
    tunnels.status = new Map(status.map((s) => [s.id, s]));
  } catch (e) {
    $("t-error").textContent = String(e);
    $("t-error").hidden = false;
    return;
  }
  const before = tunnels.byHost;
  tunnels.byHost = activeTunnelsByHost(tunnels.defs, tunnels.status);
  // Redessiner la liste d'hotes a chaque tick coûterait pour rien : on ne le
  // fait que si un badge change.
  const changed =
    before.size !== tunnels.byHost.size ||
    [...tunnels.byHost].some(([k, v]) => before.get(k) !== v);
  if (changed) renderHosts();
  if (tunnelsModal().classList.contains("open")) renderTunnels();
}

function renderTunnels() {
  const list = $("tunnel-list");
  list.innerHTML = "";
  // L'hote d'origine en tete, le reste ensuite : on voit d'abord ce pour
  // quoi on a ouvert la modale, sans perdre la vue d'ensemble.
  const defs = [...tunnels.defs].sort((a, b) => {
    const fa = a.alias === tunnels.focusAlias ? 0 : 1;
    const fb = b.alias === tunnels.focusAlias ? 0 : 1;
    return fa - fb || a.alias.localeCompare(b.alias) || a.bind_port - b.bind_port;
  });
  if (defs.length === 0) {
    const empty = document.createElement("div");
    empty.className = "tunnel-empty";
    empty.textContent = t("tunnels-aucun");
    list.appendChild(empty);
    return;
  }
  for (const d of defs) {
    const st = tunnels.status.get(d.id);
    const running = !!st;
    const alive = !!st?.alive;
    const row = document.createElement("div");
    row.className = "tunnel-row" + (running ? (alive ? " alive" : " dead") : "");
    row.innerHTML = `<span class="tdot"></span>
      <div class="tmain">
        <div class="ttitle"><span class="tflag"></span><span class="tname"></span></div>
        <div class="tdesc"></div>
        <div class="tstats"></div>
        <div class="terr" hidden></div>
      </div>
      <div class="tacts">
        <button class="tbtn" data-act="toggle"></button>
        <button class="tbtn" data-act="edit" title="Modifier">${ic("pencil")}</button>
        <button class="tbtn danger" data-act="delete" title="${t("supprimer")}">${ic("trash")}</button>
      </div>`;
    row.querySelector(".tflag")!.textContent = tunnelFlag(d.kind);
    row.querySelector(".tname")!.textContent = d.name || d.alias;
    const desc = row.querySelector(".tdesc") as HTMLElement;
    desc.textContent = describeTunnel(d);
    desc.title = describeTunnel(d); // la ligne est tronquee si longue
    const stats = row.querySelector(".tstats")!;
    if (st && alive) stats.textContent = tunnelTraffic(st);
    else if (st) stats.textContent = t("tunnels-connexion-perdue");
    else stats.textContent = t("tunnels-arrete");
    const err = row.querySelector(".terr") as HTMLElement;
    if (st?.last_error) {
      err.textContent = `⚠️ ${st.last_error}`;
      err.hidden = false;
    }
    const toggle = row.querySelector('[data-act="toggle"]') as HTMLButtonElement;
    if (tunnels.busy.has(d.id)) {
      toggle.textContent = "…";
      toggle.disabled = true;
    } else if (alive) {
      toggle.innerHTML = `${ic("stop")}<span>${t("tunnels-arreter")}</span>`;
      toggle.className = "tbtn stop labeled";
    } else {
      toggle.innerHTML = `${ic("refresh")}<span>${running ? t("tunnels-relancer") : t("tunnels-demarrer")}</span>`;
      toggle.className = "tbtn go labeled";
    }
    toggle.addEventListener("click", () => tunnelToggle(d));
    row.querySelector('[data-act="edit"]')!.addEventListener("click", () => tunnelEdit(d));
    row.querySelector('[data-act="delete"]')!.addEventListener("click", () => tunnelDelete(d));
    list.appendChild(row);
  }
}

async function tunnelToggle(d: TunnelDef) {
  const st = tunnels.status.get(d.id);
  if (st?.alive) {
    try {
      await invoke("tunnel_stop", { id: d.id });
    } catch (e) {
      notifyErreur(t("tunnels-arret-impossible", { e: String(e) }));
    }
    await tunnelsRefresh();
    return;
  }
  await tunnelStart(d);
}

/**
 * Demarre un tunnel, avec le meme dialogue de mot de passe qu'un onglet :
 * demande avant si l'hote n'a rien pour s'authentifier, redemande sur refus.
 */
async function tunnelStart(d: TunnelDef) {
  const h = state.hosts.find((x) => x.alias === d.alias);
  const label = h ? `${h.user ?? "?"}@${h.hostname ?? h.alias}:${h.port ?? 22}` : d.alias;
  let password: string | null = null;
  let rememberAsked = false;
  try {
    if (await invoke<boolean>("host_needs_password", { alias: d.alias })) {
      const rep = await askPassword(label);
      if (!rep) return;
      password = rep.password;
      rememberAsked = rep.remember;
    }
  } catch {
    /* le backend dira ce qui manque */
  }
  tunnels.busy.add(d.id);
  renderTunnels();
  try {
    for (let essai = 0; essai < 3; essai++) {
      try {
        await invoke("tunnel_start", { id: d.id, password });
        if (password && rememberAsked && h) {
          await invoke("password_save", {
            addr: h.hostname ?? h.alias,
            port: h.port,
            user: h.user ?? null,
            password,
          }).catch(() => { /* facultatif */ });
        }
        return;
      } catch (e) {
        const msg = String(e);
        if (!isPasswordRequired(msg)) {
          $("t-error").textContent = t("tunnels-demarrage-impossible", { e: msg });
          $("t-error").hidden = false;
          return;
        }
        const rep = await askPassword(label, essai === 0 ? undefined : t("mdp-refuse-nouvelle-tentative"));
        if (!rep) return;
        password = rep.password;
        rememberAsked = rep.remember;
      }
    }
  } finally {
    tunnels.busy.delete(d.id);
    await tunnelsRefresh();
  }
}

async function tunnelDelete(d: TunnelDef) {
  const ok = await askConfirm(t("tunnels-supprimer-question", { nom: d.name || describeTunnel(d) }) +
    (tunnels.status.get(d.id)?.alive ? "\n\n" + t("tunnels-actif-sera-coupe") : ""));
  if (!ok) return;
  try {
    await invoke("tunnel_def_delete", { id: d.id });
  } catch (e) {
    notifyErreur(t("suppression-impossible", { e: String(e) }));
  }
  await tunnelsRefresh();
}

// ----- Formulaire -----

const KIND_HINTS: Record<TunnelKind, { hint: string; bind: string; host: string }> = {
  local: {
    hint: t("tunnels-local-hint"),
    bind: t("port-local-d-ecoute"),
    host: t("destination-vue-du-serveur"),
  },
  remote: {
    hint: t("tunnels-distant-hint"),
    bind: t("tunnels-port-serveur"),
    host: t("tunnels-destination-machine"),
  },
  dynamic: {
    hint: t("tunnels-socks-hint"),
    bind: t("tunnels-port-mandataire"),
    host: "",
  },
};

function tunnelKind(): TunnelKind {
  const checked = document.querySelector<HTMLInputElement>('input[name="tkind"]:checked');
  return (checked?.value as TunnelKind) ?? "local";
}

function tunnelSyncKind() {
  const k = KIND_HINTS[tunnelKind()];
  $("t-kind-hint").textContent = k.hint;
  $("t-bind-label").textContent = k.bind;
  $("t-host-label").textContent = k.host;
  $("t-target-row").hidden = tunnelKind() === "dynamic";
  ($("t-bind") as HTMLInputElement).placeholder = tunnelKind() === "dynamic" ? "1080" : "8080";
}

function tunnelFormReset() {
  ($("tunnel-form") as HTMLFormElement).reset();
  ($("t-id") as HTMLInputElement).value = "";
  $("tunnel-form-title").textContent = t("nouveau-tunnel");
  $("t-submit").textContent = "Enregistrer";
  $("t-reset").hidden = true;
  $("t-error").hidden = true;
  if (tunnels.focusAlias) ($("t-alias") as HTMLSelectElement).value = tunnels.focusAlias;
  tunnelSyncKind();
}

function tunnelEdit(d: TunnelDef) {
  ($("t-id") as HTMLInputElement).value = d.id;
  ($("t-alias") as HTMLSelectElement).value = d.alias;
  (document.querySelector(`input[name="tkind"][value="${d.kind}"]`) as HTMLInputElement).checked = true;
  ($("t-bind") as HTMLInputElement).value = String(d.bind_port);
  ($("t-host") as HTMLInputElement).value = d.target_host;
  ($("t-port") as HTMLInputElement).value = d.target_port ? String(d.target_port) : "";
  ($("t-name") as HTMLInputElement).value = d.name;
  $("tunnel-form-title").textContent = `Modifier « ${d.name || describeTunnel(d)} »`;
  $("t-submit").textContent = t("enregistrer-les-modifications");
  $("t-reset").hidden = false;
  ($("tunnel-block") as HTMLDetailsElement).open = true;
  tunnelSyncKind();
  ($("t-bind") as HTMLInputElement).focus();
}

function tunnelFillHosts() {
  const sel = $("t-alias") as HTMLSelectElement;
  const current = sel.value;
  sel.innerHTML = "";
  for (const h of state.hosts) {
    const o = document.createElement("option");
    o.value = h.alias;
    o.textContent = h.alias;
    sel.appendChild(o);
  }
  if (current) sel.value = current;
}

export async function tunnelsOpen(alias?: string) {
  tunnels.focusAlias = alias ?? null;
  tunnelFillHosts();
  tunnelFormReset();
  tunnelsModal().classList.add("open");
  await tunnelsRefresh();
  renderTunnels();
  // Sans tunnel encore defini, le formulaire est la seule chose a faire :
  // on l'ouvre d'office.
  ($("tunnel-block") as HTMLDetailsElement).open = tunnels.defs.length === 0;
  if (tunnels.timer !== null) clearInterval(tunnels.timer);
  tunnels.timer = window.setInterval(tunnelsRefresh, 1500);
}

export function tunnelsClose() {
  tunnelsModal().classList.remove("open");
  if (tunnels.timer !== null) {
    clearInterval(tunnels.timer);
    tunnels.timer = null;
  }
}

$("tunnels-btn").addEventListener("click", () => tunnelsOpen());
$("t-close").addEventListener("click", tunnelsClose);
$("t-reset").addEventListener("click", tunnelFormReset);
for (const r of document.querySelectorAll('input[name="tkind"]')) {
  r.addEventListener("change", tunnelSyncKind);
}

$("tunnel-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const val = (id: string) => ($(id) as HTMLInputElement).value.trim();
  const err = $("t-error");
  const submit = $("t-submit") as HTMLButtonElement;
  const kind = tunnelKind();
  submit.disabled = true;
  try {
    await invoke("tunnel_def_save", {
      id: val("t-id") || null,
      alias: ($("t-alias") as HTMLSelectElement).value,
      kind,
      bindPort: Number(val("t-bind")),
      targetHost: kind === "dynamic" ? null : val("t-host") || null,
      targetPort: kind === "dynamic" || !val("t-port") ? null : Number(val("t-port")),
      name: val("t-name") || null,
    });
    tunnelFormReset();
    ($("tunnel-block") as HTMLDetailsElement).open = false;
    await tunnelsRefresh();
    renderTunnels();
  } catch (ex) {
    err.textContent = String(ex);
    err.hidden = false;
  } finally {
    submit.disabled = false;
  }
});

// Badges de la barre laterale : un rafraichissement initial, puis toutes les
// 5 s UNIQUEMENT s'il existe des tunnels a surveiller (sinon c'est un
// aller-retour IPC gaspille en continu au repos).
void tunnelsRefresh();
window.setInterval(() => {
  if (tunnels.defs.length === 0) return;
  if (!tunnelsModal().classList.contains("open")) void tunnelsRefresh();
}, 5000);
