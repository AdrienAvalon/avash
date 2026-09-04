//! Avash — sidecar client RDP (IronRDP), isolé de russh.
//!
//! Sert le bureau distant à Avash via un **WebSocket local binaire** (vrai
//! ArrayBuffer côté webview : pas de base64, pas de JSON — débit maximal, même
//! en 3440×1440). Écoute sur 127.0.0.1:<port aléatoire> et n'accepte qu'un
//! client présentant le bon jeton. Imprime « PORT TOKEN » sur stdout au départ.
//!
//! Messages WebSocket (binaires, auto-délimités) :
//!   sidecar -> app : [1]=CONNECTED w:u16 h:u16 · [2]=FRAME x,y,w,h:u16 + RGBA
//!                     · [7]=STATS fps:u16 kbps:u32 lat:u16 · [8]=CLIPBOARD utf8
//!                     ([3]=ERROR est réservé et géré côté front, mais nous ne
//!                      l'émettons pas : un échec avant connexion sort sur
//!                      stderr, un échec en session ferme le WebSocket et le
//!                      diagnostic est relu par `rdp_diagnostic`)
//!   app -> sidecar : [1]MOUSE_MOVE x,y · [2]BUTTON b,down,x,y · [3]WHEEL delta:i16
//!                     · [4]KEY sc:u16,down · [5]RESIZE w,h · [6]ACK · [8]CLIPBOARD utf8
//!                     · [9]REFRESH · [10]LOCKS bits:u8 · [11]PAUSE pause:u8
//!                     · [12]CLIPBOARD_AUTORISE autorise:u8
//!
//!                     · [14]KEYSYM keysym:u32,down (VNC seulement)
//!
//! Usage : avash-rdp --host H [--port 3389] -u USER -p PASS [--width W --height H] [--domain D] [--shot out.png] [--layout fr]
//!         avash-rdp --vnc --host H [--port 5900] [-u USER] (mot de passe sur stdin)

// Lints stylistiques assumés pour ce petit binaire d'orchestration :
// noms de produits en prose (doc_markdown), main() qui séquence tout le
// flux (too_many_lines), et coordonnées/RGBA aux noms courts idiomatiques.
#![allow(
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]

use crate::acces_local::Poste;
use crate::args::parse_args;
use crate::connexion::TOURS_MAX;
use crate::empreintes::chemin_canal_graphique;
use crate::session::{executer, Suite};
use anyhow::Result;

mod acces_local;
mod args;
mod atomique;
mod capture;
mod connexion;
mod egfx;
mod empreintes;
mod entrees;
mod magnetoscope;
mod presse_papiers;
mod progressif;
mod session;
mod surface;
mod trames;
mod vnc;

#[tokio::main]
async fn main() -> Result<()> {
    // Traces de diagnostic, sur une variable À NOUS et non sur RUST_LOG : beaucoup
    // l'exportent globalement, et ces traces contiennent le mot de passe en clair
    // — la requête CredSSP le porte encodé en UTF-16, lisible tel quel. Ce qui a
    // servi à trouver un défaut ne doit pas s'activer par accident.
    if let Some(filtre) = std::env::var_os("AVASH_RDP_TRACE").and_then(|v| v.into_string().ok()) {
        // Les traces contiennent le mot de passe en clair (CredSSP le porte encodé
        // en UTF-16, lisible tel quel). Elles NE VONT PAS sur stderr : depuis le
        // journal de diagnostic, l'interface capte stderr, le garde en anneau et
        // l'affiche dans l'incrustation « Connexion RDP fermée » — le mot de passe
        // se retrouverait dans une capture d'écran jointe à un rapport de bug. On
        // les écrit dans un fichier dédié en 0600 et on n'annonce sur stderr que
        // son chemin.
        // Nom IMPRÉVISIBLE (aléa 64 bits, pas seulement le PID) et ouverture en
        // create_new + O_NOFOLLOW : /tmp est mondialement inscriptible, et un nom
        // devinable ouvert en simple `create` suivrait un lien symbolique planté
        // d'avance par un autre compte — les traces, qui portent le mot de passe
        // en clair, atterriraient dans le fichier de son choix (CWE-59). create_new
        // échoue si la cible existe déjà ; O_NOFOLLOW refuse un lien.
        let chemin = std::env::temp_dir().join(format!(
            "avash-rdp-trace-{}-{:016x}.log",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut ouverture = std::fs::OpenOptions::new();
        ouverture.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            ouverture.mode(0o600);
            ouverture.custom_flags(libc::O_NOFOLLOW);
        }
        match ouverture.open(&chemin) {
            Ok(fichier) => {
                eprintln!(
                    "avash-rdp : traces actives, écrites dans {} (0600). ATTENTION, \
                     elles contiennent le mot de passe en clair — ne les collez nulle \
                     part sans les avoir relues.",
                    chemin.display()
                );
                tracing_subscriber::fmt()
                    .with_env_filter(tracing_subscriber::EnvFilter::new(filtre))
                    .with_ansi(false)
                    .with_writer(move || {
                        fichier
                            .try_clone()
                            .expect("clonage du descripteur de trace")
                    })
                    .init();
            }
            Err(e) => {
                // On refuse de retomber sur stderr : ce serait rouvrir la fuite que
                // ce fichier ferme. Sans trace, mais sans mot de passe exposé.
                eprintln!(
                    "avash-rdp : impossible d'ouvrir le fichier de trace {} ({e}) — \
                     traces désactivées.",
                    chemin.display()
                );
            }
        }
    }
    // `--rejouer <enregistrement> [--image <png>]` : rejoue sans réseau et,
    // sur demande, écrit l'image finale — le moyen de voir si un défaut
    // d'affichage vient de notre décodage.
    if let Some(chemin) = std::env::args()
        .nth(1)
        .filter(|a| a == "--rejouer")
        .and(std::env::args().nth(2))
    {
        let e = magnetoscope::lire(&chemin)?;
        let args: Vec<String> = std::env::args().collect();
        let option = |nom: &str| {
            args.iter()
                .position(|a| a == nom)
                .and_then(|i| args.get(i + 1).cloned())
        };
        // --jusqu-a N : ne rejouer que les N premiers PDU (bissection d'un défaut).
        let limite = option("--jusqu-a")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(usize::MAX);
        let (r, image) = magnetoscope::rejouer_jusqu_a(&e, false, limite)?;
        println!(
            "rejeu : {} acceptés, {} graphiques refusés, {} hors périmètre, {} rectangles, empreinte {:016x}",
            r.acceptes, r.refuses, r.hors_perimetre, r.rectangles, r.empreinte
        );
        if let Some(png) = option("--image") {
            capture::ecrire_png(&image, &png)?;
            println!("image : {png} ({}×{})", image.width(), image.height());
        }
        return Ok(());
    }
    let args = parse_args()?;
    // VNC : même poste local, même protocole avec l'interface, un autre
    // dialogue avec le serveur ; ni redirection ni canal graphique.
    if args.vnc {
        return vnc::executer(&args).await;
    }
    // Une redirection oblige à tout refaire : nouvelle connexion TCP, nouvelle
    // négociation, en présentant cette fois le jeton de routage. GNOME Remote
    // Desktop s'en sert pour remettre le client du démon système au démon de la
    // session ; sans cette boucle, on décode la demande sans pouvoir y répondre.
    //
    // Bornée à trois tours : une chaîne de redirections sans fin serait un
    // serveur mal configuré, ou hostile.
    let mut redirection: Option<Box<ironrdp::session::redirection::Redirection>> = None;
    let mut poste: Option<Poste> = None;
    let memoire = chemin_canal_graphique();
    let cle = format!("{}:{}", args.host, args.port);
    let mut graphique = egfx::Politique::pour(&cle, memoire.as_deref());
    for _ in 0..TOURS_MAX {
        let dessine = std::sync::atomic::AtomicBool::new(false);
        let issue = executer(&args, redirection.take(), &mut poste, graphique, &dessine).await;
        // Une session qui se termine sans avoir affiché la moindre image, alors
        // qu'on lui refusait le canal graphique, désigne un serveur qui n'a que
        // celui-là. GNOME Remote Desktop ne patiente même pas : son pipeline ne
        // pouvant s'ouvrir, il raccroche aussitôt. Reprendre est la seule
        // réponse juste — et la seule qui n'exige pas de deviner à l'avance à
        // quelle famille de serveur on parle.
        let issue = match issue {
            Ok(Suite::Fini) | Err(_)
                if graphique == egfx::Politique::Observer
                    && !dessine.load(std::sync::atomic::Ordering::Relaxed) =>
            {
                Ok(Suite::ReprendreAvecGraphique)
            }
            autre => autre,
        };
        match issue? {
            Suite::ReprendreAvecGraphique => {
                eprintln!(
                    "egfx : ce serveur ne dessine pas par le chemin classique, \
                     reprise avec le canal graphique"
                );
                if let Some(m) = memoire.as_deref() {
                    egfx::memoriser(&cle, m);
                }
                graphique = egfx::Politique::Accepter;
            }
            Suite::Fini => return Ok(()),
            Suite::Rediriger(r) => {
                eprintln!(
                    "redirection : jeton de {} octets, identifiants {}",
                    r.jeton.as_ref().map_or(0, Vec::len),
                    if r.utilisateur.is_some() {
                        "fournis"
                    } else {
                        "absents"
                    }
                );
                redirection = Some(r);
            }
        }
    }
    anyhow::bail!(
        "Le serveur nous redirige sans fin : {TOURS_MAX} tours n'ont pas suffi à ouvrir une session."
    )
}
