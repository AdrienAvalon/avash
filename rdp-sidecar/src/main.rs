//! Avash — sidecar client RDP (IronRDP), isolé de russh.
//!
//! Sert le bureau distant à Avash via un **WebSocket local binaire** (vrai
//! ArrayBuffer côté webview : pas de base64, pas de JSON — débit maximal, même
//! en 3440×1440). Écoute sur 127.0.0.1:<port aléatoire> et n'accepte qu'un
//! client présentant le bon jeton. Imprime « PORT TOKEN » sur stdout au départ.
//!
//! Messages WebSocket (binaires, auto-délimités) :
//!   sidecar -> app : [1]=CONNECTED w:u16 h:u16 · [2]=FRAME x,y,w,h:u16 + RGBA
//!                     · [13]=FRAMES n:u8 puis n × (x,y,w,h:u16 + RGBA)
//!                     · [7]=STATS fps:u16 kbps:u32 lat:u16 · [8]=CLIPBOARD utf8
//!                     · [14]=PRESSE-PAPIERS DISTANT VIDÉ (sans charge)
//!                     · [15]/[17]/[18] fichiers du presse-papiers (JSON, voir plus bas)
//!                     · [20]=SON (onde PCM, voir son.rs) · [21]=VOLUME
//!                     · [23]=REPRISE (sans charge) : un tour de redirection ou
//!                       de reprise avec le canal graphique commence après un
//!                       premier [1] ; l'onglet repasse en « connexion » jusqu'au
//!                       [1] suivant (audit du 12 septembre 2026, C-SIL-10)
//!                     ([3]=ERROR est réservé et géré côté front, mais nous ne
//!                      l'émettons pas : un échec avant connexion sort sur
//!                      stderr, un échec en session ferme le WebSocket et le
//!                      diagnostic est relu par `rdp_diagnostic`)
//!   app -> sidecar : [1]MOUSE_MOVE x,y · [2]BUTTON b,down,x,y · [3]WHEEL delta:i16
//!                     · [4]KEY sc:u16,down · [5]RESIZE w,h · [6]ACK · [8]CLIPBOARD utf8
//!                     · [9]REFRESH · [10]LOCKS bits:u8 · [11]PAUSE pause:u8
//!                     · [12]CLIPBOARD_AUTORISE autorise:u8
//!                     · [14]KEYSYM keysym:u32,down (VNC seulement)
//!                     · [22]COLLER (VNC seulement : pousse le presse-papiers mémorisé)
//!                     · [16]RECEVOIR json {dossier?} · [19]OFFRIR json [chemins]
//!                     (fichiers par le presse-papiers ; en retour
//!                      [15]FICHIERS_DISTANTS, [17]FICHIERS_PROGRESSION,
//!                      [18]FICHIERS_TERMINE, en JSON)
//!
//! Usage : avash-rdp --host H [--port 3389] -u USER [--width W --height H] [--domain D] [--shot out.png] [--layout fr] [--sans-nla] [--tls-herite]
//!         avash-rdp --vnc --host H [--port 5900] [-u USER]
//!         Le mot de passe se lit toujours sur la première ligne de l'entrée
//!         standard, jamais en argument (audit du 12 septembre 2026). Hors
//!         `--shot`, la fin de l'entrée standard (le parent a disparu) met fin
//!         au processus.

// Lints stylistiques assumés pour ce petit binaire d'orchestration :
// noms de produits en prose (doc_markdown), main() qui séquence tout le
// flux (too_many_lines), et coordonnées/RGBA aux noms courts idiomatiques.
#![allow(
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]

// Les modules vivent désormais dans la bibliothèque `avash_rdp` (voir
// `src/lib.rs`) : le binaire n'en est qu'un appelant. Trouvé par l'audit du
// 7 septembre 2026.
use anyhow::Result;
use avash_rdp::acces_local::Poste;
use avash_rdp::args::parse_args;
use avash_rdp::connexion::{FermeeApresAuthentification, TOURS_MAX};
use avash_rdp::empreintes::chemin_canal_graphique;
use avash_rdp::session::{executer, Suite};
use avash_rdp::{capture, egfx, fichiers, magnetoscope, vnc};

/// Sous cargo-llvm-cov (`cfg(coverage)`, posé par lui seul), le profil
/// d'exécution est réécrit toutes les secondes. Le profil ne s'écrit
/// normalement qu'à la sortie du processus ; or la suite bout en bout, seul
/// test à traverser la boucle de session, arrête ce processus quand
/// l'application meurt, souvent sans sortie propre. Sans ce fil, cette
/// couverture-là resterait à zéro (scripts/couverture.sh). Jamais dans un
/// binaire publié.
#[cfg(coverage)]
fn ecrire_le_profil_en_continu() {
    extern "C" {
        fn __llvm_profile_write_file() -> i32;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        // SAFETY: fonction du runtime de profilage, liée dès que le binaire
        // est instrumenté ; sans argument ni état partagé avec nous.
        let _ = unsafe { __llvm_profile_write_file() };
    });
}

/// Faut-il reprendre la connexion en accordant le canal graphique ?
///
/// Trouvé par l'audit du 7 septembre 2026 : la boucle reprenait sur `Err(_)`,
/// donc pour TOUT échec (mot de passe refusé, délai NLA, certificat changé,
/// TCP refusé), rejouant une seconde connexion et écrivant l'hôte dans
/// `rdp_canal_graphique`. La reprise n'est légitime que si aucune image n'a
/// été dessinée alors qu'on refusait le canal graphique (`Observer`), ET que
/// la session s'est réellement terminée sans dessin : soit `Suite::Fini`, soit
/// une fermeture par le serveur APRÈS authentification
/// (`FermeeApresAuthentification`, le cas GNOME Remote Desktop). Un échec
/// pré-session ne doit ni mémoriser ni relancer.
fn faut_il_reprendre(issue: &Result<Suite>, graphique: egfx::Politique, dessine: bool) -> bool {
    if graphique != egfx::Politique::Observer || dessine {
        return false;
    }
    match issue {
        Ok(Suite::Fini) => true,
        Err(e) => e.downcast_ref::<FermeeApresAuthentification>().is_some(),
        Ok(_) => false,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(coverage)]
    ecrire_le_profil_en_continu();

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
                // Un seul descripteur, partagé sous verrou. Trouvé par l'audit du
                // 12 septembre 2026 (C-panique-8) : la fermeture dupliquait le
                // descripteur à CHAQUE événement et paniquait si la duplication
                // échouait (descripteurs épuisés), tuant le processus au moment
                // même où l'on cherchait à comprendre un défaut.
                tracing_subscriber::fmt()
                    .with_env_filter(tracing_subscriber::EnvFilter::new(filtre))
                    .with_ansi(false)
                    .with_writer(std::sync::Mutex::new(fichier))
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
    // Le mot de passe consommé (première ligne), stdin porte ensuite les
    // chemins que l'utilisateur désigne, annoncés par le parent : seuls ceux-là
    // pourront être offerts au distant (voir `fichiers::Designations`). Un fil
    // ordinaire, bloqué en lecture : ce flux ne presse jamais.
    //
    // Sa fin veut dire que le parent a disparu, et le processus avec lui. Trouvé
    // par l'audit du 12 septembre 2026 (C-sidecar-1) : elle ne faisait que tarir
    // les désignations, si bien qu'une application tombée entre le lancement et
    // la connexion de sa WebSocket laissait une session RDP authentifiée ouverte
    // sur le serveur, sans personne. Sauf en capture d'écran (`--shot`), lancée
    // à la main ou par un script qui ferme stdin juste après le mot de passe
    // (scripts/conformite.sh, scripts/tracer-rdp.sh) : là, rien ne l'attend
    // sur stdin et le délai de connexion borne déjà le processus.
    let lie_au_parent = args.lie_au_parent();
    std::thread::spawn(move || {
        use std::io::BufRead as _;
        for ligne in std::io::stdin().lock().lines().map_while(Result::ok) {
            if let Some(chemin) = fichiers::designation_depuis_ligne(&ligne) {
                fichiers::DESIGNATIONS.designer(chemin);
            }
        }
        if lie_au_parent {
            eprintln!(
                "avash-rdp : entrée standard fermée, l'application a disparu : fin du processus."
            );
            std::process::exit(0);
        }
    });
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
        let issue = if faut_il_reprendre(
            &issue,
            graphique,
            dessine.load(std::sync::atomic::Ordering::Relaxed),
        ) {
            Ok(Suite::ReprendreAvecGraphique)
        } else {
            issue
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

#[cfg(test)]
mod tests_reprise {
    use super::{faut_il_reprendre, FermeeApresAuthentification};
    use anyhow::Result;
    use avash_rdp::egfx::Politique;
    use avash_rdp::session::Suite;

    // Cas confirmés par l'audit du 7 septembre 2026 : la décision de reprendre
    // avec le canal graphique ne doit dépendre QUE d'une session réellement
    // ouverte puis fermée sans dessin, jamais d'un échec pré-session.

    /// Un mot de passe refusé (CredSSP STATUS_LOGON_FAILURE) est un échec
    /// pré-session : ne ni mémoriser ni relancer, sinon deux tentatives d'auth
    /// avec le même mot de passe faux (double 4625 côté serveur).
    #[test]
    fn un_mot_de_passe_faux_ne_declenche_pas_de_reprise() {
        let issue: Result<Suite> = Err(anyhow::anyhow!("CredSSP … STATUS_LOGON_FAILURE"));
        assert!(!faut_il_reprendre(&issue, Politique::Observer, false));
    }

    /// Un délai NLA dépassé n'a jamais ouvert de session : pas de reprise.
    #[test]
    fn un_timeout_nla_ne_declenche_pas_de_reprise() {
        let issue: Result<Suite> = Err(anyhow::anyhow!(
            "[AVASH_RDP_SANS_NLA] L'authentification réseau (NLA) n'a pas abouti"
        ));
        assert!(!faut_il_reprendre(&issue, Politique::Observer, false));
    }

    /// Un certificat changé (TOFU) fait échouer AVANT tout envoi
    /// d'identifiants : pas de reprise, pas d'écriture dans rdp_canal_graphique.
    #[test]
    fn un_certificat_change_ne_declenche_pas_de_reprise() {
        let issue: Result<Suite> = Err(anyhow::anyhow!("Le certificat de srv:3389 a changé."));
        assert!(!faut_il_reprendre(&issue, Politique::Observer, false));
    }

    /// Contrôle positif : un serveur qui n'a que le canal graphique et
    /// raccroche juste après l'authentification (GNOME Remote Desktop) doit,
    /// lui, reprendre.
    #[test]
    fn serveur_egfx_only_qui_raccroche_reprend() {
        let issue: Result<Suite> = Err(anyhow::Error::new(FermeeApresAuthentification(
            "session fermée après auth".to_owned(),
        )));
        assert!(faut_il_reprendre(&issue, Politique::Observer, false));
    }

    /// Une fin de session sans dessin, sous Observer, reste le cas légitime.
    #[test]
    fn une_session_finie_sans_dessin_reprend() {
        let issue: Result<Suite> = Ok(Suite::Fini);
        assert!(faut_il_reprendre(&issue, Politique::Observer, false));
    }

    /// Mais si quelque chose a été dessiné, le serveur sait dessiner par le
    /// chemin classique : aucune reprise, même sur une fin sans plus.
    #[test]
    fn une_session_qui_a_dessine_ne_reprend_pas() {
        let issue: Result<Suite> = Ok(Suite::Fini);
        assert!(!faut_il_reprendre(&issue, Politique::Observer, true));
    }

    /// Et sous une politique autre qu'Observer (le canal est déjà accordé),
    /// il n'y a rien à reprendre, même sur le marqueur d'après-auth.
    #[test]
    fn hors_observer_aucune_reprise() {
        let issue: Result<Suite> = Err(anyhow::Error::new(FermeeApresAuthentification(
            "session fermée après auth".to_owned(),
        )));
        assert!(!faut_il_reprendre(&issue, Politique::Accepter, false));
    }
}
