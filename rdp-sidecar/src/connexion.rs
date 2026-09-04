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
use tokio::net::TcpStream;

fn build_config(
    a: &Args,
    redirection: Option<&ironrdp::session::redirection::Redirection>,
) -> connector::Config {
    let (username, domain) = split_credentials(&a.user, a.domain.as_deref());
    connector::Config {
        // Après une redirection, le serveur impose SES identifiants — engendrés
        // pour l'occasion — et non ceux de l'utilisateur. C'est ainsi que GNOME
        // remet la connexion d'un démon à l'autre.
        credentials: match redirection {
            Some(r) if r.utilisateur.is_some() => Credentials::UsernamePassword {
                username: r.utilisateur.clone().unwrap_or_default(),
                password: r
                    .mot_de_passe
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(o).into_owned())
                    .unwrap_or_default(),
            },
            _ => Credentials::UsernamePassword {
                username,
                password: a.pass.clone(),
            },
        },
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
        desktop_scale_factor: 0,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        alternate_shell: String::new(),
        work_dir: String::new(),
    }
}

/// Rectangle mis à jour -> message FRAME binaire [2][x][y][w][h][RGBA].
/// Marqueur reconnu par l'interface : le serveur ne sait pas faire de NLA.
///
/// Elle propose alors de se connecter quand même, en expliquant ce que cela
/// coûte, et retient le choix pour ce serveur. Un marqueur plutôt qu'un texte
/// anglais issu d'une dépendance : celui-ci ne changera pas sous nos pieds.
pub const NLA_INDISPONIBLE: &str = "[AVASH_RDP_SANS_NLA]";

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

pub(crate) async fn connect(
    a: &Args,
    clip_backend: ClipBackend,
    son: Option<crate::son::SonBackend>,
    redirection: Option<&ironrdp::session::redirection::Redirection>,
    graphique: egfx::Politique,
) -> Result<(
    connector::ConnectionResult,
    ironrdp_tokio::TokioFramed<ironrdp_tls::TlsStream<TcpStream>>,
    egfx::CanalPartage,
    egfx::FilePartagee,
)> {
    let tcp = TcpStream::connect((a.host.as_str(), a.port))
        .await
        .with_context(|| format!("connexion TCP à {}:{}", a.host, a.port))?;
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
    let (mut upgraded_stream, cert) =
        ironrdp_tls::upgrade(initial, &a.host).await.map_err(|e| {
            // Le serveur a accepté la négociation, puis rompu pendant TLS.
            // Sous Windows cela remonte en « os error 10054 », un code brut que
            // rien ne permet d'interpréter — signalé par Adrien sur un Windows
            // Server. Renoncer à NLA n'y changerait rien : ce repli passe lui
            // aussi par TLS. Le message doit donc envoyer chercher ailleurs.
            if est_coupure(&chaine_des_causes(&e)) {
                anyhow::anyhow!(
                    "Ce serveur a accepté la négociation puis a rompu la connexion \
                     pendant l'établissement du canal chiffré. C'est le plus souvent \
                     un certificat RDP absent ou abîmé côté serveur, ou une couche \
                     de sécurité réglée sur « RDP » au lieu de « SSL ». Renoncer à \
                     l'authentification réseau n'y changerait rien : ce repli passe \
                     lui aussi par TLS."
                )
            } else {
                anyhow::Error::new(e).context("passage TLS")
            }
        })?;
    let pubkey = server_public_key(&cert)?;

    // TOFU sur le certificat, AVANT CredSSP : c'est CredSSP qui transmet les
    // identifiants. Vérifier après reviendrait à les avoir déjà livrés.
    let cle = format!("{}:{}", a.host, a.port);
    let presentee = empreinte(&pubkey);
    match juger_certificat(empreinte_memorisee(&cle).as_deref(), &presentee) {
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

    // Connexion redirigée : l'authentification RDSTLS vient ici, APRÈS la
    // vérification du certificat — elle transporte des identifiants, et les
    // livrer à un serveur non vérifié annulerait la protection qu'on vient
    // d'appliquer.
    if let Some(r) = redirection {
        rdstls_authentifier(&mut upgraded_stream, r).await?;
    }

    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);
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
            anyhow::anyhow!(
                "Le serveur a accepté vos identifiants puis a mis fin à la session \
                 avant de l'ouvrir. L'authentification n'est pas en cause : c'est \
                 côté serveur que la session ne démarre pas, et il ne dit pas \
                 pourquoi. Sur un hôte Linux, son journal le dira — \
                 /var/log/xrdp-sesman.log."
            )
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
pub(crate) const TOURS_MAX: usize = 6;

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
        assert!(
            verdict_rdstls(0x0000_052e).contains("pas une \u{fffd}rreur de saisie")
                || verdict_rdstls(0x0000_052e).contains("erreur de saisie")
        );
    }
}

#[cfg(test)]
mod tests_configuration {
    use super::build_config;
    use crate::args::parse_args_de;
    use ironrdp::connector::Credentials;
    use ironrdp::session::redirection::Redirection;

    fn redirection() -> Redirection {
        Redirection {
            session_id: 7,
            drapeaux: 0,
            adresse: None,
            jeton: Some(b"Cookie: msts=2464288595\r\n".to_vec()),
            utilisateur: Some("69<;349v".to_owned()),
            domaine: None,
            mot_de_passe: Some(b"secret".to_vec()),
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
