//! Connexion : configuration IronRDP, négociation (NLA, RDSTLS), redirections, classification des coupures.

use crate::acces_local::valeur_du_jeton;
use crate::args::{split_credentials, Args};
use crate::egfx;
use crate::empreintes::{
    empreinte, empreinte_memorisee, juger_certificat, memoriser_empreinte, server_public_key,
    VerdictCert,
};
use crate::presse_papiers::ClipBackend;
use anyhow::{Context, Result};
use ironrdp::cliprdr::CliprdrClient;
use ironrdp::connector::{self, Credentials};
use ironrdp::displaycontrol::client::DisplayControlClient;
use ironrdp::dvc::DrdynvcClient;
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;
use ironrdp::pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use std::time::Duration;
use tokio::net::TcpStream;

/// Drapeau MS-RDPBCGR 2.2.13.1.1 : le mot de passe de la redirection est chiffré
/// par la clé publique du serveur d'arrivée. Il ne sert alors qu'à RDSTLS, qui le
/// transporte tel quel ; CredSSP ne saurait qu'en faire.
const LB_PASSWORD_IS_PK_ENCRYPTED: u32 = 0x0001_0000;

/// Décode de l'UTF-16 petit-boutien (le mot de passe en clair d'une redirection),
/// en s'arrêtant au terminateur nul. Le PDU de redirection porte ses chaînes en
/// UTF-16LE : les décoder en UTF-8 donnerait un mot de passe faux.
fn utf16le_vers_string(o: &[u8]) -> String {
    let mots: Vec<u16> = o
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u16::from_le_bytes(*p))
        .take_while(|c| *c != 0)
        .collect();
    String::from_utf16_lossy(&mots)
}

fn build_config(
    a: &Args,
    redirection: Option<&ironrdp::session::redirection::Redirection>,
) -> connector::Config {
    let (username, domain) = split_credentials(&a.user, a.domain.as_deref());
    // Identité effective après une éventuelle redirection. Le serveur d'arrivée
    // impose SES nom d'utilisateur et domaine — engendrés pour l'occasion : c'est
    // ainsi que GNOME remet la connexion d'un démon à l'autre.
    //
    // Le mot de passe demande plus de soin. Trouvé par l'audit du 7 septembre
    // 2026 : si le serveur d'arrivée retient HYBRID plutôt que RDSTLS (un hôte
    // RDS Windows derrière un broker, quand la redirection ne porte pas de mot de
    // passe), c'est CredSSP qui part — avec CES identifiants. Or le mot de passe
    // fourni par la redirection est chiffré par la clé publique du serveur
    // (LB_PASSWORD_IS_PK_ENCRYPTED) et ne sert qu'à RDSTLS, qui le transporte tel
    // quel : CredSSP ne saurait qu'en faire. On ne réutilise donc le mot de passe
    // de la redirection que s'il est présent ET en clair (UTF-16LE) ; sinon on
    // retombe sur le mot de passe saisi. Sans quoi CredSSP partait avec un mot de
    // passe vide et le domaine tapé (et non celui de la redirection) —
    // STATUS_LOGON_FAILURE, et une tentative échouée journalisée sur l'hôte cible.
    let (username, domain, password) = match redirection {
        Some(r) => (
            r.utilisateur.clone().unwrap_or(username),
            r.domaine.clone().or(domain),
            match (
                r.mot_de_passe.as_deref(),
                r.drapeaux & LB_PASSWORD_IS_PK_ENCRYPTED,
            ) {
                (Some(p), 0) => utf16le_vers_string(p),
                _ => a.pass.clone(),
            },
        ),
        None => (username, domain, a.pass.clone()),
    };
    connector::Config {
        credentials: Credentials::UsernamePassword { username, password },
        domain,
        // `enable_tls` annonce PROTOCOL_SSL au serveur, ce qui — la
        // documentation d'ironrdp le dit mot pour mot — revient à **accepter le
        // repli de NLA vers TLS seul**. Un serveur qui répond « SSL » voyait
        // alors CredSSP sauté (connection.rs : « CredSSP is disabled, skipping
        // NLA ») et le mot de passe partait dans le Client Info PDU, sans
        // authentification mutuelle. C'est précisément au premier contact —
        // le seul moment où le TOFU ne protège pas — que cela coûte le plus.
        // En n'annonçant que HYBRID, un serveur incapable de NLA fait échouer
        // la négociation, ce qui est le bon comportement.
        //
        // `--sans-nla` rétablit l'annonce de SSL, **sur décision explicite de
        // l'utilisateur** et pour ce serveur-là seulement : certains serveurs
        // légitimes n'offrent pas NLA — un xrdp dont le module PAM n'est pas
        // configuré, par exemple. On annonce alors les deux, et le serveur
        // choisit : NLA reste préféré s'il sait le faire.
        enable_tls: a.sans_nla,
        enable_credssp: true,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: a.layout,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: connector::DesktopSize {
            width: a.width,
            height: a.height,
        },
        bitmap: None,
        client_build: 0,
        client_name: "avash-rdp".to_owned(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".to_owned(),
        platform: MajorPlatformType::UNIX,
        enable_server_pointer: false,
        // Le jeton de routage réoriente la connexion vers la bonne session ;
        // sans lui, le serveur nous renverrait à l'accueil, indéfiniment.
        request_data: redirection
            .and_then(|r| r.jeton.as_deref())
            .map(valeur_du_jeton)
            .map(ironrdp::pdu::nego::NegoRequestData::routing_token),
        autologon: false,
        enable_audio_playback: false,
        compression_type: None,
        pointer_software_rendering: true,
        multitransport_flags: None,
        performance_flags: PerformanceFlags::default(),
        // Échelle DPI annoncée au serveur (`desktopScaleFactor`, MS-RDPBCGR).
        // L'interface la calcule depuis `devicePixelRatio` et la passe par
        // `--scale` : sur un écran à 200 %, on négocie la définition en pixels
        // physiques ET on annonce 200, sinon le serveur rendrait son interface à
        // 100 % — texte net mais deux fois trop petit. Ajouté par l'audit du
        // 7 septembre 2026 (HiDPI) ; 0 quand l'écran est standard.
        desktop_scale_factor: a.desktop_scale_factor,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        alternate_shell: String::new(),
        work_dir: String::new(),
    }
}

/// Marqueur reconnu par l'interface : le serveur ne sait pas faire de NLA.
///
/// Elle propose alors de se connecter quand même, en expliquant ce que cela
/// coûte, et retient le choix pour ce serveur. Un marqueur plutôt qu'un texte
/// anglais issu d'une dépendance : celui-ci ne changera pas sous nos pieds.
pub const NLA_INDISPONIBLE: &str = "[AVASH_RDP_SANS_NLA]";

/// Marqueur : le serveur a fermé la session APRÈS nous avoir authentifiés,
/// avant qu'elle ne s'ouvre.
///
/// Trouvé par l'audit du 7 septembre 2026 : `main` reprenait « avec canal
/// graphique » sur `Err(_)`, c'est-à-dire pour TOUT échec de `executer` — un
/// mot de passe refusé, un délai NLA dépassé, un certificat TOFU changé, une
/// connexion TCP refusée. Il rejouait alors une seconde connexion (doublant
/// l'événement 4625 côté serveur avec le même mot de passe faux) et polluait
/// `rdp_canal_graphique`. Seule la fermeture par le serveur APRÈS
/// authentification, sans le moindre dessin, désigne un serveur qui n'a que
/// le canal graphique (GNOME Remote Desktop raccroche ainsi) : ce marqueur
/// distingue ce cas des échecs pré-session, pour que `faut_il_reprendre` ne
/// reprenne que là. Il porte le message affiché à l'utilisateur.
#[derive(Debug)]
pub struct FermeeApresAuthentification(pub String);

impl std::fmt::Display for FermeeApresAuthentification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FermeeApresAuthentification {}

/// Le serveur a-t-il mis fin à la session après nous avoir authentifiés ?
///
/// Deux formes pour un même événement, et c'est ce qui a trompé Adrien :
///
/// - le serveur envoie un *Disconnect Provider Ultimatum* et nous le lisons ;
/// - il coupe la connexion TCP, et c'est le système qui nous le dit —
///   « connection reset by peer » sous Unix, **os error 10054** sous Windows,
///   qui ne ressemble à rien pour qui le lit.
///
/// Le second cas affichait un code brut là où le premier expliquait. Même
/// cause, même message.
pub(crate) fn session_close_par_le_serveur(texte: &str) -> bool {
    texte.contains("disconnect provider ultimatum") || est_coupure(texte)
}

/// La connexion a-t-elle été coupée brutalement, sans réponse ?
///
/// Sous Windows cela remonte en `os error 10054` (WSAECONNRESET), un code brut
/// qui ne dit rien à qui le lit. Sous Unix, `os error 104`. Une fermeture nette
/// en cours de lecture donne, elle, une fin de flux inattendue.
fn coupure_brutale(e: &connector::ConnectorError) -> bool {
    est_coupure(&chaine_des_causes(e))
}

/// Aplatit un message et toute sa chaîne de causes.
///
/// La phrase utile vit rarement dans l'affichage direct : elle est enfouie dans
/// les causes. Sans ce parcours, la détection ne voit rien.
fn chaine_des_causes(e: &(dyn std::error::Error + 'static)) -> String {
    let mut texte = format!("{e} {e:?}");
    let mut source = e.source();
    while let Some(c) = source {
        texte.push(' ');
        texte.push_str(&c.to_string());
        source = c.source();
    }
    texte
}

/// Le pair a-t-il coupé sans rien dire ?
///
/// Windows remonte `os error 10054` (WSAECONNRESET), Unix `os error 104`. Ces
/// codes bruts ne disent rien à qui les reçoit — c'est exactement ce qu'Adrien a
/// vu en tentant un RDP vers un Windows.
fn est_coupure(texte: &str) -> bool {
    texte.contains("os error 10054")
        || texte.contains("os error 104")
        || texte.contains("connection reset")
        || texte.contains("Connection reset")
        || texte.contains("unexpected end of file")
        || texte.contains("early eof")
        || texte.contains("custom error")
}

/// Version et types de PDU RDSTLS (MS-RDPBCGR 2.2.17).
const RDSTLS_VERSION_1: u16 = 0x0001;

const RDSTLS_TYPE_CAPABILITIES: u16 = 0x0001;

const RDSTLS_TYPE_AUTHREQ: u16 = 0x0002;

const RDSTLS_TYPE_AUTHRSP: u16 = 0x0004;

const RDSTLS_DATA_PASSWORD_CREDS: u16 = 0x0001;

/// Traduit le verdict du serveur d'arrivée.
///
/// Ces identifiants sont engendrés par le serveur lui-même et n'ont qu'un
/// usage : un refus ne vient donc jamais d'une faute de frappe de
/// l'utilisateur, et le message ne doit pas le lui laisser croire.
fn verdict_rdstls(code: u32) -> String {
    let raison = match code {
        0x0000_0005 => "le compte n'a pas le droit d'accéder à ce serveur",
        0x0000_052e => "le serveur d'arrivée ne reconnaît pas les identifiants transmis",
        0x0000_0530 => "le compte est soumis à des plages horaires",
        0x0000_0532 => "le mot de passe du compte a expiré",
        0x0000_0533 => "le compte est désactivé",
        0x0000_0773 => "le mot de passe du compte doit être changé",
        0x0000_0775 => "le compte est verrouillé",
        _ => "raison inconnue",
    };
    format!(
        "Le serveur d'arrivée a refusé la redirection : {raison} (code {code:#010x}). \
         Ces identifiants sont engendrés par le serveur lui-même : ce n'est pas une \
         erreur de saisie, mais un désaccord entre ses deux démons — ou une \
         redirection expirée."
    )
}

/// Authentification RDSTLS (MS-RDPBCGR 2.2.17), après la montée TLS.
///
/// C'est le protocole des connexions **redirigées**. Le serveur d'arrivée
/// n'attend ni CredSSP ni TLS simple : il veut qu'on lui réémette, tels quels,
/// les champs que la redirection nous a remis — identifiant de redirection,
/// nom d'utilisateur, domaine et mot de passe. Ce dernier est chiffré par clé
/// publique ; le client ne le déchiffre pas, il le transporte.
///
/// Sans cet échange, la séquence se poursuit puis le serveur met fin à la
/// session — ce qui ressemble à s'y méprendre à un refus de session.
async fn rdstls_authentifier<S>(
    flux: &mut S,
    r: &ironrdp::session::redirection::Redirection,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    /// Longueur sur 16 bits, puis les octets tels quels.
    fn champ(m: &mut Vec<u8>, v: Option<&Vec<u8>>) {
        let v = v.map_or(&[][..], Vec::as_slice);
        m.extend_from_slice(&u16::try_from(v.len()).unwrap_or(0).to_le_bytes());
        m.extend_from_slice(v);
    }

    // Le serveur parle le premier : il annonce ses capacités. Huit octets —
    // Version, PduType, DataType, VersionsPrises — et non six comme on pourrait
    // le déduire d'une lecture rapide de la spécification. Vérifié sur le fil.
    let mut capacites = [0u8; 8];
    flux.read_exact(&mut capacites)
        .await
        .context("capacités RDSTLS")?;
    let type_capacites = u16::from_le_bytes([capacites[2], capacites[3]]);
    anyhow::ensure!(
        type_capacites == RDSTLS_TYPE_CAPABILITIES,
        "Réponse RDSTLS inattendue : type {type_capacites}, capacités attendues."
    );

    let mut m = Vec::with_capacity(256);
    m.extend_from_slice(&RDSTLS_VERSION_1.to_le_bytes());
    m.extend_from_slice(&RDSTLS_TYPE_AUTHREQ.to_le_bytes());
    m.extend_from_slice(&RDSTLS_DATA_PASSWORD_CREDS.to_le_bytes());
    champ(&mut m, r.guid.as_ref());
    champ(&mut m, r.utilisateur_brut.as_ref());
    champ(&mut m, r.domaine_brut.as_ref());
    champ(&mut m, r.mot_de_passe.as_ref());
    flux.write_all(&m).await.context("envoi RDSTLS")?;
    flux.flush().await.ok();

    // Verdict : Version, PduType, DataType, puis le code sur quatre octets.
    let mut rep = [0u8; 10];
    flux.read_exact(&mut rep).await.context("réponse RDSTLS")?;
    let type_reponse = u16::from_le_bytes([rep[2], rep[3]]);
    anyhow::ensure!(
        type_reponse == RDSTLS_TYPE_AUTHRSP,
        "Réponse RDSTLS inattendue : type {type_reponse}, verdict attendu."
    );
    let code = u32::from_le_bytes([rep[6], rep[7], rep[8], rep[9]]);
    anyhow::ensure!(code == 0, "{}", verdict_rdstls(code));
    Ok(())
}

/// TOFU sur le certificat du serveur RDP, AVANT CredSSP : premier contact
/// mémorisé, certificat connu accepté, certificat changé refusé sans que rien ne
/// soit réécrit dans le fichier des empreintes.
///
/// Extrait de `connect` pour être éprouvable sans une vraie poignée IronRDP
/// (trouvé par l'audit du 7 septembre 2026 : le montage complet n'était testé
/// que pour VNC via `vnc_tls::monter` ; côté RDP, rien ne verrouillait le format
/// de la clé d'épinglage — « hôte:port » NUE, sans préfixe, contrairement à
/// « vnc:hôte:port », et à ne surtout pas préfixer sous peine de réapprendre en
/// silence toutes les empreintes déjà mémorisées — ni le refus au changement,
/// qu'un `memoriser_empreinte` glissé à la place du `bail!` aurait mué en
/// réapprentissage muet).
fn epingler_certificat(hote: &str, port: u16, pubkey: &[u8]) -> Result<()> {
    let cle = format!("{hote}:{port}");
    let presentee = empreinte(pubkey);
    // Un fichier de confiance illisible (droits, ou UTF-8 invalide) est un refus
    // explicite, pas un « premier contact » : voir `empreinte_memorisee`.
    let memorisee =
        empreinte_memorisee(&cle).context("fichier de confiance illisible, connexion refusée")?;
    match juger_certificat(memorisee.as_deref(), &presentee) {
        VerdictCert::Connu => {}
        VerdictCert::PremierContact => memoriser_empreinte(&cle, &presentee)
            .context("mémorisation de l'empreinte du serveur RDP")?,
        VerdictCert::Change { attendue } => {
            anyhow::bail!(
                "Le certificat de {cle} a changé.\n\nSoit le serveur a été \
                 réinstallé, soit quelqu'un intercepte la connexion.\n\n\
                 Empreinte présentée : {presentee}\nEmpreinte attendue  : {attendue}\n\n\
                 Si le changement est légitime, retirez la ligne « {cle} » de \
                 rdp_known_hosts."
            );
        }
    }
    Ok(())
}

/// Délai propre à la phase TCP, distinct du délai global de la session (25 s).
///
/// Trouvé par l'audit du 7 septembre 2026 : `connect` était enveloppé en entier
/// dans l'unique délai de 25 s de `session::executer`, la connexion TCP comprise.
/// Or un hôte dont les SYN restent sans réponse (pare-feu en DROP, machine
/// éteinte hors du LAN, route noire) n'échoue au niveau du noyau qu'après ~127 s
/// (`net.ipv4.tcp_syn_retries = 6`) : c'était donc le délai de 25 s qui tombait,
/// et la boucle de session émettait « NLA n'a pas abouti » avec le marqueur
/// `NLA_INDISPONIBLE` qui pousse l'interface à proposer de renoncer à NLA (choix
/// mémorisé par serveur) — alors qu'aucun octet n'avait été échangé et que NLA
/// n'avait jamais commencé. On borne donc la connexion TCP à part, avec un
/// message neutre et SANS le marqueur ; le délai de 25 s ne couvre plus que
/// TLS, TOFU et CredSSP, dont le message NLA reste juste.
const DELAI_TCP: Duration = Duration::from_secs(10);

/// Établit la connexion TCP en la bornant par `delai`. Le futur de connexion est
/// injecté pour que le test puisse exercer le dépassement sans hôte réel. En cas
/// de dépassement, l'erreur est neutre et ne porte PAS le marqueur
/// `NLA_INDISPONIBLE` : une panne TCP pure n'est pas un échec d'authentification.
async fn connecter_tcp(
    host: &str,
    port: u16,
    delai: Duration,
    connexion: impl std::future::Future<Output = std::io::Result<TcpStream>>,
) -> Result<TcpStream> {
    match tokio::time::timeout(delai, connexion).await {
        Ok(r) => r.with_context(|| format!("connexion TCP à {host}:{port}")),
        Err(_) => anyhow::bail!(
            "Le serveur {host}:{port} ne répond pas (aucune réponse TCP en {} s) : \
             vérifiez l'adresse, le réseau et le pare-feu.",
            delai.as_secs()
        ),
    }
}

pub(crate) async fn connect(
    a: &Args,
    clip_backend: ClipBackend,
    son: Option<crate::son::SonBackend>,
    disque: Option<crate::disque::DisqueBackend>,
    redirection: Option<&ironrdp::session::redirection::Redirection>,
    graphique: egfx::Politique,
) -> Result<(
    connector::ConnectionResult,
    ironrdp_tokio::TokioFramed<crate::tls_herite::Flux>,
    egfx::CanalPartage,
    egfx::FilePartagee,
)> {
    let tcp = connecter_tcp(
        &a.host,
        a.port,
        DELAI_TCP,
        TcpStream::connect((a.host.as_str(), a.port)),
    )
    .await?;
    // Nagle OFF : les entrées et les petits rectangles d'écran partent sans délai.
    tcp.set_nodelay(true).ok();
    let client_addr = tcp.local_addr()?;
    let mut framed = ironrdp_tokio::TokioFramed::new(tcp);
    let (egfx, canal_egfx, file_egfx) = egfx::Egfx::nouveau();
    // Canal Display Control (DVC) : permet le redimensionnement natif du
    // bureau distant (le serveur re-rend à la nouvelle résolution).
    let mut dvc = DrdynvcClient::new()
        .with_dynamic_channel(DisplayControlClient::new(|_caps| Ok(Vec::new())));
    // Le canal graphique n'est offert qu'aux serveurs qui ont montré n'en avoir
    // pas d'autre : l'accepter suffit à faire taire un serveur Windows. Voir
    // `egfx::Politique`.
    if graphique == egfx::Politique::Accepter {
        dvc.attach_dynamic_channel(egfx);
    }
    let mut connector = connector::ClientConnector::new(build_config(a, redirection), client_addr)
        .with_static_channel(dvc)
        // Canal CLIPRDR : presse-papiers partagé poste <-> bureau distant (texte).
        .with_static_channel(CliprdrClient::new(Box::new(clip_backend)));
    // Canal RDPSND : le son du distant, joué par la webview. Pas annoncé quand
    // l'utilisateur l'a coupé (--sans-son) : un canal absent ne coûte rien au
    // serveur, un canal muet lui ferait encoder pour personne.
    if let Some(son) = son {
        connector =
            connector.with_static_channel(ironrdp::rdpsnd::client::Rdpsnd::new(Box::new(son)));
    }
    // Canal RDPDR : le dossier partagé, servi comme lecteur « Avash ». Un
    // lecteur est un périphérique « après ouverture de session » : le canal
    // l'annonce de lui-même quand le serveur dit l'utilisateur connecté.
    if let Some(disque) = disque {
        connector = connector.with_static_channel(
            ironrdp::rdpdr::Rdpdr::new(Box::new(disque), "avash-rdp".to_owned()).with_drives(Some(
                vec![(
                    crate::disque::LECTEUR_ID,
                    crate::disque::LECTEUR_NOM.to_owned(),
                )],
            )),
        );
    }
    let should_upgrade = match ironrdp_tokio::connect_begin(&mut framed, &mut connector).await {
        Ok(v) => v,
        // Le serveur a refusé la négociation alors que nous n'annoncions que
        // NLA : il ne sait pas le faire. Ce n'est pas forcément une attaque —
        // un xrdp sans module PAM est dans ce cas — mais ce n'est pas à nous
        // d'en décider en silence. On remonte un marqueur que l'interface
        // reconnaît, pour poser la question à l'utilisateur.
        Err(e)
            if !a.sans_nla && matches!(e.kind(), connector::ConnectorErrorKind::Negotiation(_)) =>
        {
            anyhow::bail!(
                "{NLA_INDISPONIBLE} Ce serveur n'accepte pas l'authentification \
                 réseau (NLA) et exige un simple canal TLS."
            );
        }
        // Coupure brutale pendant la négociation. Windows la remonte comme
        // « os error 10054 », qui ne dit rien à personne — Adrien l'a reçue tel
        // quel. Un serveur qui ferme sans répondre est le plus souvent un
        // serveur qui ne sait pas faire ce qu'on lui demande : ici, NLA. On pose
        // donc la même question que pour un refus explicite, en disant
        // clairement ce qu'on sait et ce qu'on ignore.
        Err(e) if !a.sans_nla && coupure_brutale(&e) => {
            anyhow::bail!(
                "{NLA_INDISPONIBLE} Ce serveur a fermé la connexion sans répondre \
                 à la négociation. C'est le comportement de serveurs qui n'acceptent \
                 pas l'authentification réseau (NLA) — mais un pare-feu ou un service \
                 qui n'est pas du RDP donneraient la même chose."
            );
        }
        Err(e) if coupure_brutale(&e) => {
            anyhow::bail!(
                "Ce serveur a fermé la connexion sans répondre. Vérifiez que le \
                 service RDP écoute bien sur ce port et qu'aucun pare-feu ne s'y \
                 oppose."
            );
        }
        Err(e) => return Err(e).context("début de connexion"),
    };
    let initial = framed.into_inner_no_leftover();
    // Deux piles pour monter le canal chiffré. rustls par défaut ; celle du
    // système sur `--tls-herite`, décision explicite de l'utilisateur pour un
    // serveur sans suite moderne (Windows Server 2012 R2 et antérieurs, voir
    // `tls_herite`). Le serveur a accepté la négociation, puis rompu pendant
    // TLS : sous Windows cela remontait en « os error 10054 », un code brut que
    // rien ne permettait d'interpréter, signalé par Adrien sur un Windows
    // Server. Renoncer à NLA n'y changerait rien : ce repli passe lui aussi
    // par TLS. Le message doit donc dire ce qu'il reste à essayer.
    let (mut upgraded_stream, cert) = if a.tls_herite {
        crate::tls_herite::monter(initial, &a.host)
            .await
            .map_err(|e| {
                if est_coupure(&format!("{e:#}")) {
                    anyhow::anyhow!("{}", crate::tls_herite::message_coupure(true))
                } else {
                    e.context("passage TLS hérité")
                }
            })?
    } else {
        match ironrdp_tls::upgrade(initial, &a.host).await {
            Ok((flux, cert)) => (crate::tls_herite::Flux::Moderne(Box::new(flux)), cert),
            Err(e) if est_coupure(&chaine_des_causes(&e)) => {
                anyhow::bail!("{}", crate::tls_herite::message_coupure(false));
            }
            Err(e) => return Err(anyhow::Error::new(e).context("passage TLS")),
        }
    };
    let pubkey = server_public_key(&cert)?;

    // TOFU sur le certificat, AVANT CredSSP : c'est CredSSP qui transmet les
    // identifiants. Vérifier après reviendrait à les avoir déjà livrés.
    epingler_certificat(&a.host, a.port, &pubkey)?;

    // `mark_as_upgraded` ne fait AUCUNE E/S : il ne fait qu'avancer la machine
    // d'états (EnhancedSecurityUpgrade -> Credssp si le serveur a retenu
    // HYBRID|HYBRID_EX, sinon BasicSettings). On le fait donc AVANT de décider de
    // RDSTLS, pour pouvoir interroger `should_perform_credssp()`.
    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);

    // Connexion redirigée : RDSTLS ne se joue que si le serveur d'arrivée ne
    // réclame pas CredSSP, et APRÈS la vérification du certificat — il transporte
    // des identifiants, les livrer à un serveur non vérifié annulerait la
    // protection qu'on vient d'appliquer.
    //
    // Trouvé par l'audit du 7 septembre 2026 : le connecteur porté annonce RDSTLS
    // EN PLUS de HYBRID|HYBRID_EX (enable_credssp reste vrai) et c'est le serveur
    // qui tranche. On jouait RDSTLS dès qu'une redirection était en cours, sans
    // regarder ce choix : si le serveur retenait HYBRID (un hôte RDS Windows
    // derrière un broker, quand la redirection ne porte pas de mot de passe
    // chiffré par clé publique), `rdstls_authentifier` bloquait sur son
    // `read_exact(8)` en attendant des capacités que le serveur — qui attend, lui,
    // notre premier TSRequest — n'enverrait jamais : blocage mutuel jusqu'au délai
    // de 25 s, puis « NLA n'a pas abouti ». `should_perform_credssp()` reflète le
    // choix du serveur une fois `mark_as_upgraded` passé.
    if let Some(r) = redirection {
        if !connector.should_perform_credssp() {
            rdstls_authentifier(&mut upgraded_stream, r).await?;
        }
    }

    let mut framed = ironrdp_tokio::TokioFramed::new(upgraded_stream);
    let mut net = ironrdp_tokio::reqwest::ReqwestNetworkClient::new();
    let result = ironrdp_tokio::connect_finalize(
        upgraded,
        connector,
        &mut framed,
        &mut net,
        a.host.clone().into(),
        pubkey,
        None,
    )
    .await
    .map_err(|e| {
        // Cette étape couvre TOUTE la fin de séquence, pas seulement NLA :
        // licence, capacités, activation. Un serveur qui coupe après avoir
        // accepté les identifiants tombait ici sous l'étiquette « CredSSP/NLA »,
        // qui accusait l'authentification alors qu'elle avait réussi.
        // La phrase vit dans la CHAÎNE de causes, pas dans l'affichage direct :
        // il faut la parcourir, sinon la détection ne voit rien.
        let mut texte = format!("{e} {e:?}");
        let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&e);
        while let Some(c) = source {
            texte.push(' ');
            texte.push_str(&c.to_string());
            source = c.source();
        }
        if session_close_par_le_serveur(&texte) {
            // Marqueur distinct : c'est le SEUL échec qui justifie de reprendre
            // avec le canal graphique (voir `FermeeApresAuthentification`).
            anyhow::Error::new(FermeeApresAuthentification(
                "Le serveur a accepté vos identifiants puis a mis fin à la session \
                 avant de l'ouvrir. L'authentification n'est pas en cause : c'est \
                 côté serveur que la session ne démarre pas, et il ne dit pas \
                 pourquoi. Sur un hôte Linux, son journal le dira — \
                 /var/log/xrdp-sesman.log."
                    .to_owned(),
            ))
        } else {
            anyhow::Error::new(e).context("fin de la séquence de connexion")
        }
    })?;
    Ok((result, framed, canal_egfx, file_egfx))
}

/// Nombre de connexions successives tolérées pour une seule ouverture de
/// session. Quatre suffisent au pire cas connu : connexion, redirection, reprise
/// avec le canal graphique, redirection de nouveau. La marge est là pour ne pas
/// transformer un serveur inhabituel en échec ; la borne, pour qu'un serveur qui
/// redirige en rond ne nous y entraîne pas.
pub const TOURS_MAX: usize = 6;

#[cfg(test)]
mod tests_negociation {
    use super::build_config;
    use crate::args::parse_args_de;

    /// Par défaut, seul NLA est annoncé : un serveur qui ne sait pas le faire
    /// doit échouer la négociation, pas obtenir le mot de passe dans un canal
    /// TLS sans s'être authentifié.
    #[test]
    fn par_defaut_seul_nla_est_annonce() {
        let a = parse_args_de(&["--host", "x", "-u", "u"], "p").unwrap();
        let c = build_config(&a, None);
        assert!(
            !c.enable_tls,
            "SSL annoncé : le repli de NLA vers TLS redevient possible"
        );
        assert!(c.enable_credssp);
    }

    /// `--sans-nla` rétablit l'annonce de SSL — sur décision explicite de
    /// l'utilisateur, pour un serveur qui ne propose pas NLA (un xrdp dont le
    /// module PAM n'est pas configuré, par exemple). NLA reste préféré si le
    /// serveur sait le faire : on annonce les deux, il choisit.
    #[test]
    fn sans_nla_annonce_les_deux_sans_renoncer_a_nla() {
        let a = parse_args_de(&["--host", "x", "-u", "u", "--sans-nla"], "p").unwrap();
        let c = build_config(&a, None);
        assert!(c.enable_tls);
        assert!(
            c.enable_credssp,
            "NLA doit rester préféré quand le serveur sait le faire"
        );
    }
}

#[cfg(test)]
mod tests_fin_de_session {
    use super::session_close_par_le_serveur;

    #[test]
    fn l_ultimatum_est_reconnu() {
        assert!(session_close_par_le_serveur(
            "decode error other (received disconnect provider ultimatum)"
        ));
    }

    #[test]
    fn la_coupure_tcp_windows_est_reconnue() {
        // 10054 = WSAECONNRESET. Le même événement que l'ultimatum, mais vu par
        // le système : c'est le code brut qu'Adrien a reçu sous Windows.
        assert!(session_close_par_le_serveur(
            "lecture PDU: Une connexion existante a dû être fermée (os error 10054)"
        ));
    }

    #[test]
    fn la_coupure_tcp_unix_est_reconnue() {
        assert!(session_close_par_le_serveur(
            "lecture PDU: Connection reset by peer (os error 104)"
        ));
    }

    #[test]
    fn une_erreur_sans_rapport_ne_l_est_pas() {
        // Sans quoi tout échec porterait un message rassurant et faux.
        assert!(!session_close_par_le_serveur(
            "InvalidToken: CredSSP server returned an error status; status is STATUS_LOGON_FAILURE"
        ));
        assert!(!session_close_par_le_serveur(
            "connexion TCP à 10.0.0.1:3389: timed out"
        ));
    }
}

#[cfg(test)]
mod tests_coupure {
    use super::est_coupure;

    #[test]
    fn le_code_windows_est_reconnu() {
        // 10054 = WSAECONNRESET : le code brut qu'un utilisateur reçoit sans
        // pouvoir en rien conclure.
        assert!(est_coupure(
            "début de connexion: Une connexion existante a dû être fermée (os error 10054)"
        ));
    }

    #[test]
    fn le_code_unix_et_la_fin_de_flux_sont_reconnus() {
        assert!(est_coupure("Connection reset by peer (os error 104)"));
        assert!(est_coupure("unexpected end of file"));
    }

    #[test]
    fn un_echec_ordinaire_ne_l_est_pas() {
        // Sans quoi un mauvais mot de passe proposerait de renoncer à NLA.
        assert!(!est_coupure("STATUS_LOGON_FAILURE"));
        assert!(!est_coupure("connexion TCP à 10.0.0.1:3389: timed out"));
        assert!(!est_coupure("Le certificat de 10.0.0.1:3389 a changé."));
    }
}

#[cfg(test)]
mod tests_delai_tcp {
    use super::{connecter_tcp, NLA_INDISPONIBLE};
    use std::time::Duration;

    /// Trouvé par l'audit du 7 septembre 2026 : un hôte dont les SYN restent
    /// sans réponse (machine éteinte derrière un pare-feu en DROP) faisait tomber
    /// le délai global de 25 s de la session, qui diagnostiquait alors « NLA n'a
    /// pas abouti » avec le marqueur `NLA_INDISPONIBLE` — poussant l'interface à
    /// proposer de renoncer à NLA sur une panne TCP pure, où aucun octet n'a
    /// circulé. Le futur de connexion est ici simulé par un `pending` qui ne
    /// répond jamais : le dépassement doit produire un message NEUTRE, sans le
    /// marqueur, disant que le serveur ne répond pas.
    #[tokio::test]
    async fn un_hote_muet_ne_propose_pas_de_renoncer_a_nla() {
        let erreur = connecter_tcp(
            "10.0.0.9",
            3389,
            Duration::from_millis(50),
            std::future::pending(),
        )
        .await
        .expect_err("un hôte qui ne répond jamais doit dépasser le délai");
        let message = format!("{erreur:#}");
        assert!(
            !message.contains(NLA_INDISPONIBLE),
            "une panne TCP ne doit pas porter le marqueur NLA : {message}"
        );
        assert!(
            message.contains("ne répond pas") && message.contains("10.0.0.9:3389"),
            "le message doit désigner l'hôte injoignable : {message}"
        );
    }

    /// La connexion qui aboutit avant le délai passe telle quelle : le
    /// bornage n'avale pas le succès. On simule un `TcpStream` réel en se
    /// connectant à un écouteur local ouvert pour l'occasion.
    #[tokio::test]
    async fn une_connexion_qui_aboutit_passe() {
        let ecouteur = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("écouteur local");
        let adresse = ecouteur.local_addr().expect("adresse locale");
        let flux = connecter_tcp(
            "127.0.0.1",
            adresse.port(),
            Duration::from_secs(5),
            tokio::net::TcpStream::connect(adresse),
        )
        .await
        .expect("la connexion locale doit aboutir");
        assert_eq!(flux.peer_addr().expect("pair").port(), adresse.port());
    }
}

#[cfg(test)]
mod tests_rdstls {
    use super::verdict_rdstls;

    #[test]
    fn les_codes_connus_sont_traduits() {
        assert!(verdict_rdstls(0x0000_052e).contains("ne reconnaît pas les identifiants"));
        assert!(verdict_rdstls(0x0000_0775).contains("verrouillé"));
    }

    #[test]
    fn un_code_inconnu_reste_lisible() {
        let m = verdict_rdstls(0x0000_dead);
        assert!(m.contains("raison inconnue"));
        assert!(
            m.contains("0x0000dead"),
            "le code brut doit rester consultable : {m}"
        );
    }

    #[test]
    fn le_message_decharge_l_utilisateur() {
        // Ces identifiants sont engendrés par le serveur : accuser une faute de
        // frappe enverrait chercher au mauvais endroit.
        // Trouvé par l'audit du 7 septembre 2026 : l'assertion contenait un
        // U+FFFD (« pas une �rreur »), résidu d'un ré-encodage abîmé, qui ne
        // pouvait jamais correspondre ; l'alternative de repli « erreur de
        // saisie » restait vraie même pour un message qui accuse l'utilisateur.
        // On exige désormais la négation entière.
        let m = verdict_rdstls(0x0000_052e);
        assert!(m.contains("pas une erreur de saisie"), "{m}");
    }
}

#[cfg(test)]
mod tests_epinglage {
    use super::epingler_certificat;

    /// `AVASH_HOME` posé sur un répertoire jetable le temps du test, sous le
    /// verrou que partagent tous les tests touchant cette variable globale, et
    /// remis en place à la sortie même sur panique. Sans lui, le test écrirait
    /// dans le fichier de confiance RÉEL du poste.
    struct Bac {
        chemin: std::path::PathBuf,
        precedent: Option<std::ffi::OsString>,
        _verrou: std::sync::MutexGuard<'static, ()>,
    }

    impl Bac {
        fn poser() -> Self {
            let verrou = crate::empreintes::VERROU_AVASH_HOME
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let chemin =
                std::env::temp_dir().join(format!("avash-rdp-tofu-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&chemin);
            let precedent = std::env::var_os("AVASH_HOME");
            unsafe { std::env::set_var("AVASH_HOME", &chemin) };
            Self {
                chemin,
                precedent,
                _verrou: verrou,
            }
        }

        fn fichier_de_confiance(&self) -> std::path::PathBuf {
            self.chemin
                .join(".config")
                .join("avash")
                .join("rdp_known_hosts")
        }
    }

    impl Drop for Bac {
        fn drop(&mut self) {
            unsafe {
                match self.precedent.take() {
                    Some(v) => std::env::set_var("AVASH_HOME", v),
                    None => std::env::remove_var("AVASH_HOME"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.chemin);
        }
    }

    /// Clé publique jetable, distincte à chaque appel (certificat auto-signé
    /// neuf) : deux appels donnent deux empreintes, comme un serveur réinstallé.
    /// On passe par `server_public_key`, le même chemin que `connect`.
    fn cle_publique_jetable() -> Vec<u8> {
        use x509_cert::der::Decode as _;
        let cle = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let der = cle.cert.der().to_vec();
        let cert = x509_cert::Certificate::from_der(&der).unwrap();
        crate::empreintes::server_public_key(&cert).unwrap()
    }

    /// Le montage complet du TOFU RDP n'était éprouvé nulle part — seul VNC
    /// l'était (`vnc_tls::tests::le_montage_epingle_le_certificat_et_refuse_qu_il_change`).
    /// Trouvé par l'audit du 7 septembre 2026. On rejoue ici la même séquence :
    /// premier contact mémorisé sous « hôte:port » NUE (sans préfixe, à la
    /// différence de « vnc:hôte:port » — un préfixe RDP réapprendrait en silence
    /// toutes les empreintes déjà mémorisées), même clé reconnue sans rien
    /// réécrire, clé changée refusée avec les deux empreintes, et l'empreinte
    /// d'origine conservée. Contrôle négatif : remplacer le `bail!` par un
    /// `memoriser_empreinte` (réapprentissage silencieux), préfixer la clé ou en
    /// retirer le port fait rougir ce test.
    #[test]
    fn l_epinglage_memorise_puis_refuse_le_changement() {
        let bac = Bac::poser();
        let port = 3389u16;
        let origine = cle_publique_jetable();
        let remplacant = cle_publique_jetable();

        epingler_certificat("127.0.0.1", port, &origine).expect("le premier contact est accepté");
        let contenu = std::fs::read_to_string(bac.fichier_de_confiance()).unwrap();
        assert!(
            contenu.starts_with(&format!("127.0.0.1:{port} ")),
            "l'empreinte est mémorisée sous la clé « hôte:port » NUE : {contenu:?}"
        );
        assert!(
            !contenu.contains("rdp:"),
            "la clé RDP ne porte aucun préfixe, sous peine de tout réapprendre en silence : {contenu:?}"
        );

        epingler_certificat("127.0.0.1", port, &origine).expect("la même clé est reconnue");
        assert_eq!(
            std::fs::read_to_string(bac.fichier_de_confiance()).unwrap(),
            contenu,
            "un serveur connu ne fait rien réécrire"
        );

        let refus = epingler_certificat("127.0.0.1", port, &remplacant)
            .expect_err("une clé changée est refusée");
        let msg = format!("{refus:#}");
        for attendu in [
            "a changé",
            "Empreinte présentée",
            "Empreinte attendue",
            "rdp_known_hosts",
        ] {
            assert!(msg.contains(attendu), "{attendu:?} absent de {msg:?}");
        }
        assert_eq!(
            std::fs::read_to_string(bac.fichier_de_confiance()).unwrap(),
            contenu,
            "le refus ne touche pas à l'empreinte d'origine"
        );
    }
}

#[cfg(test)]
mod tests_configuration {
    use super::{build_config, LB_PASSWORD_IS_PK_ENCRYPTED};
    use crate::args::parse_args_de;
    use ironrdp::connector::Credentials;
    use ironrdp::pdu::nego::NegoRequestData;
    use ironrdp::session::redirection::Redirection;

    /// Encode une chaîne en UTF-16LE avec terminateur nul, comme le fait un PDU
    /// de redirection.
    fn u16le(s: &str) -> Vec<u8> {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    fn redirection() -> Redirection {
        Redirection {
            session_id: 7,
            drapeaux: 0,
            adresse: None,
            jeton: Some(b"Cookie: msts=2464288595\r\n".to_vec()),
            utilisateur: Some("69<;349v".to_owned()),
            domaine: None,
            // Le PDU porte le mot de passe en UTF-16LE, pas en UTF-8.
            mot_de_passe: Some(u16le("secret")),
            fqdn: None,
            guid: None,
            utilisateur_brut: None,
            domaine_brut: None,
        }
    }

    /// Après une redirection, ce sont les identifiants du serveur — engendrés
    /// pour l'occasion — qui partent, et le jeton de routage est replacé dans
    /// la requête X.224. Sans quoi GNOME Remote Desktop renvoie à l'accueil,
    /// indéfiniment.
    #[test]
    fn une_redirection_impose_ses_identifiants_et_son_jeton() {
        let a = parse_args_de(&["--host", "x", "-u", "adrien"], "mdp").unwrap();
        let c = build_config(&a, Some(&redirection()));
        match c.credentials {
            Credentials::UsernamePassword { username, password } => {
                assert_eq!(username, "69<;349v");
                assert_eq!(password, "secret");
            }
            autre => panic!("identifiants inattendus : {autre:?}"),
        }
        assert!(
            c.request_data.is_some(),
            "le jeton de routage doit être posé"
        );
    }

    /// Trouvé par l'audit du 7 septembre 2026 : quand le serveur d'arrivée
    /// retient HYBRID plutôt que RDSTLS, CredSSP part avec les identifiants de
    /// `build_config`. Une redirection de ferme RDS derrière un broker ne porte
    /// pas de mot de passe (LB_USERNAME + LB_DOMAIN sans LB_PASSWORD) : CredSSP
    /// doit alors réutiliser le mot de passe saisi, et le domaine de la
    /// redirection — et non un mot de passe vide avec le domaine tapé, qui
    /// donnait STATUS_LOGON_FAILURE et une tentative journalisée côté cible.
    #[test]
    fn un_repli_hybrid_sans_mot_de_passe_reutilise_le_mot_de_passe_saisi() {
        let mut r = redirection();
        r.utilisateur = Some("svc-rds".to_owned());
        r.domaine = Some("RDSFARM".to_owned());
        r.mot_de_passe = None;
        let a = parse_args_de(&["--host", "x", "-u", "TAPE\\adrien"], "mdp").unwrap();
        let c = build_config(&a, Some(&r));
        match c.credentials {
            Credentials::UsernamePassword { username, password } => {
                assert_eq!(username, "svc-rds");
                assert_eq!(
                    password, "mdp",
                    "le mot de passe saisi doit servir à CredSSP"
                );
            }
            autre => panic!("identifiants inattendus : {autre:?}"),
        }
        assert_eq!(
            c.domain.as_deref(),
            Some("RDSFARM"),
            "le domaine de la redirection l'emporte sur celui tapé"
        );
    }

    /// Le mot de passe d'une redirection chiffré par clé publique
    /// (LB_PASSWORD_IS_PK_ENCRYPTED) ne sert qu'à RDSTLS, qui le transporte tel
    /// quel. Il ne doit jamais atterrir dans les identifiants CredSSP : sur un
    /// repli HYBRID, c'est le mot de passe saisi qui part.
    #[test]
    fn un_mot_de_passe_de_redirection_chiffre_ne_sert_pas_a_credssp() {
        let mut r = redirection();
        r.drapeaux |= LB_PASSWORD_IS_PK_ENCRYPTED;
        r.mot_de_passe = Some(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let a = parse_args_de(&["--host", "x", "-u", "adrien"], "mdp").unwrap();
        let c = build_config(&a, Some(&r));
        match c.credentials {
            Credentials::UsernamePassword { password, .. } => {
                assert_eq!(
                    password, "mdp",
                    "le blob chiffré ne doit pas servir à CredSSP"
                );
            }
            autre => panic!("identifiants inattendus : {autre:?}"),
        }
    }

    /// Une redirection annonce RDSTLS EN PLUS de HYBRID (enable_credssp reste
    /// vrai) : le serveur peut donc retenir HYBRID. C'est ce qui rend le
    /// garde-fou `should_perform_credssp()` de `connect` nécessaire — sans lui,
    /// RDSTLS serait joué à tort et bloquerait.
    #[test]
    fn une_redirection_annonce_toujours_hybrid() {
        let a = parse_args_de(&["--host", "x", "-u", "adrien"], "mdp").unwrap();
        let c = build_config(&a, Some(&redirection()));
        assert!(c.enable_credssp, "HYBRID doit rester annoncé");
        assert!(
            matches!(c.request_data, Some(NegoRequestData::RoutingToken(_))),
            "le jeton de routage annonce aussi RDSTLS : le serveur tranche"
        );
    }

    #[test]
    fn sans_redirection_ce_sont_ceux_de_l_utilisateur() {
        let a = parse_args_de(&["--host", "x", "-u", "TEST\\adrien"], "mdp").unwrap();
        let c = build_config(&a, None);
        match c.credentials {
            Credentials::UsernamePassword { username, password } => {
                assert_eq!(username, "adrien");
                assert_eq!(password, "mdp");
            }
            autre => panic!("identifiants inattendus : {autre:?}"),
        }
        assert_eq!(c.domain.as_deref(), Some("TEST"));
        assert!(c.request_data.is_none());
    }
}
