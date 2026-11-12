//! The interface's language. Screens write their text in English, wrapped in a lookup: `t("Continue watching")`, or
//! `tr!("Season {}", n)` for text with values in it. The table below has every key in the other five languages, and
//! English is what shows for anything it lacks. The language is a global read as each frame draws, so a change in
//! Settings shows everywhere at once: screens keep keys (or a [`Text`]), never words already translated. It's the
//! TV's own menu language unless Settings say otherwise.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::{self, Display, Write};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, OnceLock};

use super::languages::language_name;

/// The languages the interface is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Lang {
    En,
    Es,
    /// Brazilian Portuguese
    Pt,
    Fr,
    De,
    It,
}

impl Lang {
    /// In the table's order.
    pub const ALL: [Lang; 6] = [Lang::En, Lang::Es, Lang::Pt, Lang::Fr, Lang::De, Lang::It];

    /// What the setting stores.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Es => "es",
            Lang::Pt => "pt",
            Lang::Fr => "fr",
            Lang::De => "de",
            Lang::It => "it",
        }
    }

    /// Its name in itself.
    pub fn endonym(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Es => "Español",
            Lang::Pt => "Português",
            Lang::Fr => "Français",
            Lang::De => "Deutsch",
            Lang::It => "Italiano",
        }
    }

    /// Crunchyroll's locale for it, for what Crunchyroll words itself (the avatars' albums).
    pub fn locale(self) -> &'static str {
        match self {
            Lang::En => "en-US",
            Lang::Es => "es-419",
            Lang::Pt => "pt-BR",
            Lang::Fr => "fr-FR",
            Lang::De => "de-DE",
            Lang::It => "it-IT",
        }
    }

    /// The language of a locale ("pt-BR", "en_US", "es"), when the interface has it.
    pub fn from_code(code: &str) -> Option<Lang> {
        let language = code.split(['-', '_']).next()?.trim().to_ascii_lowercase();
        Lang::ALL.into_iter().find(|lang| lang.code() == language)
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::En as u8);

/// The language the interface is in now.
pub fn lang() -> Lang {
    Lang::ALL[CURRENT.load(Ordering::Relaxed) as usize]
}

/// The TV's menu language, read once (English when the interface hasn't got it, or it can't be read).
pub fn tv_lang() -> Lang {
    static TV: OnceLock<Lang> = OnceLock::new();
    *TV.get_or_init(|| crate::platform::ui_locale().as_deref().and_then(Lang::from_code).unwrap_or(Lang::En))
}

/// Puts the interface in the language a setting names: "auto", or anything it doesn't know, is the TV's.
pub fn apply(setting: &str) {
    let lang = Lang::from_code(setting).unwrap_or_else(tv_lang);
    CURRENT.store(lang as u8, Ordering::Relaxed);
    eprintln!("crunchy: interface in {} (the TV's in {})", lang.code(), tv_lang().code());
}

/// `key` in the interface's language: from the table, else the key itself (English, or a key it lacks).
pub fn t(key: &str) -> &str {
    translate(lang(), key)
}

fn translate(lang: Lang, key: &str) -> &str {
    match lang {
        Lang::En => key,
        _ => TABLE.get(key).map_or(key, |row| row[lang as usize]),
    }
}

/// `template` with each `{}` replaced by the next of `args`: what [`tr!`] does after the lookup.
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut args = args.iter();
    let mut rest = template;
    while let Some(at) = rest.find("{}") {
        out.push_str(&rest[..at]);
        if let Some(arg) = args.next() {
            let _ = write!(out, "{arg}");
        }
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

/// A key with `{}` where values go, looked up and filled in: `tr!("Season {}", 2)` is "Temporada 2" in Spanish.
macro_rules! tr {
    ($key:literal $(, $arg:expr)* $(,)?) => {
        $crate::app::i18n::fill($crate::app::i18n::t($key), &[$(&$arg as &dyn ::std::fmt::Display),*])
    };
}
pub(crate) use tr;

/// Whether a count takes the singular key ("{} season"): 1, and in French 0 as well.
pub fn singular(count: u32) -> bool {
    singular_in(lang(), count)
}

fn singular_in(lang: Lang, count: u32) -> bool {
    match lang {
        Lang::Fr => count <= 1,
        _ => count == 1,
    }
}

/// A date the short way: 04/10/25, the month first in English (10/04/25), with dots in German (04.10.25).
pub fn short_date(year: i32, month: u32, day: u32) -> String {
    short_date_in(lang(), year, month, day)
}

fn short_date_in(lang: Lang, year: i32, month: u32, day: u32) -> String {
    let year = year.rem_euclid(100);
    match lang {
        Lang::En => format!("{month:02}/{day:02}/{year:02}"),
        Lang::De => format!("{day:02}.{month:02}.{year:02}"),
        _ => format!("{day:02}/{month:02}/{year:02}"),
    }
}

/// A number with one decimal and the language's decimal mark: "4.8", "4,8".
pub fn one_decimal(value: f32) -> String {
    let text = format!("{value:.1}");
    if lang() == Lang::En { text } else { text.replace('.', ",") }
}

/// Text kept to draw later: a key, looked up each time it's drawn (so it follows the language), a language named in
/// the interface's, or text that isn't ours to translate (a show's title, a genre, an error from the network).
#[derive(Clone, Debug, PartialEq)]
pub enum Text {
    Key(&'static str),
    /// a language's code ("ja-JP")
    Language(String),
    Plain(String),
}

impl Text {
    /// What it says in the interface's language now.
    pub fn get(&self) -> Cow<'_, str> {
        match self {
            Text::Key(key) => Cow::Borrowed(t(key)),
            Text::Language(code) => Cow::Owned(language_name(code)),
            Text::Plain(text) => Cow::Borrowed(text),
        }
    }
}

/// In English, for the log. It also makes a [`Text`] an error the worker can hand the screen to show in its language.
impl Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Text::Key(key) => f.write_str(key),
            Text::Language(text) | Text::Plain(text) => f.write_str(text),
        }
    }
}

/// The table, by key.
static TABLE: LazyLock<HashMap<&'static str, &'static [&'static str; 6]>> =
    LazyLock::new(|| ENTRIES.iter().map(|row| (row[0], row)).collect());

/// Each key (English) with its Spanish (Latin America), Brazilian Portuguese, French, German and Italian.
#[rustfmt::skip]
static ENTRIES: &[[&str; 6]] = &[
    // the dialog
    ["OK", "Aceptar", "OK", "OK", "OK", "OK"],
    ["Cancel", "Cancelar", "Cancelar", "Annuler", "Abbrechen", "Annulla"],
    // signing in
    ["Sign in", "Inicia sesión", "Faça login", "Connectez-vous", "Melde dich an", "Accedi"],
    ["1. On your phone or computer, go to", "1. En tu móvil o computadora, entra en", "1. No celular ou computador, acesse",
        "1. Sur votre téléphone ou ordinateur, allez sur", "1. Öffne auf dem Handy oder Computer", "1. Dal telefono o dal computer, vai su"],
    ["2. Enter this code:", "2. Escribe este código:", "2. Digite este código:", "2. Saisissez ce code\u{a0}:", "2. Gib diesen Code ein:",
        "2. Inserisci questo codice:"],
    ["The screen will move on by itself once you enter it.", "La pantalla avanzará sola en cuanto lo escribas.",
        "A tela vai avançar sozinha assim que você digitar.", "L’écran passera à la suite tout seul dès que vous l’aurez saisi.",
        "Der Bildschirm geht von selbst weiter, sobald du ihn eingegeben hast.", "La schermata andrà avanti da sola appena lo inserisci."],
    ["The code expired. Press OK to get a new one.", "El código caducó. Pulsa OK para pedir otro.",
        "O código expirou. Pressione OK para gerar outro.", "Le code a expiré. Appuyez sur OK pour en obtenir un autre.",
        "Der Code ist abgelaufen. Drücke OK für einen neuen.", "Il codice è scaduto. Premi OK per riceverne un altro."],
    // the profiles
    ["Who’s going on an adventure?", "¿Quién se irá de aventura?", "Quem vai embarcar na aventura?", "Qui part à l’aventure\u{a0}?",
        "Wer zieht los ins Abenteuer?", "Chi parte all’avventura?"],
    ["Add", "Agregar", "Adicionar", "Ajouter", "Hinzufügen", "Aggiungi"],
    ["Exit the app?", "¿Salir de la aplicación?", "Sair do aplicativo?", "Quitter l’application\u{a0}?", "App beenden?", "Uscire dall’app?"],
    ["Sign Out", "Cerrar Sesión", "Sair da conta", "Se déconnecter", "Abmelden", "Esci dall’account"],
    ["Are you sure you want to sign out?", "¿Estás seguro de que quieres cerrar sesión?", "Tem certeza de que quer sair da conta?",
        "Voulez-vous vraiment vous déconnecter\u{a0}?", "Möchtest du dich wirklich abmelden?", "Vuoi davvero uscire dall’account?"],
    // a profile's form
    ["Avatar", "Avatar", "Avatar", "Avatar", "Avatar", "Avatar"],
    ["Name", "Nombre", "Nome", "Nom", "Name", "Nome"],
    ["Username", "Nombre de usuario", "Nome de usuário", "Nom d’utilisateur", "Benutzername", "Nome utente"],
    ["Content language", "Idioma del contenido", "Idioma do conteúdo", "Langue du contenu", "Sprache der Inhalte", "Lingua dei contenuti"],
    ["Audio language", "Idioma de audio", "Idioma do áudio", "Langue audio", "Audiosprache", "Lingua audio"],
    ["Subtitle language", "Idioma de subtítulos", "Idioma das legendas", "Langue des sous-titres", "Untertitelsprache", "Lingua dei sottotitoli"],
    ["Save", "Guardar", "Salvar", "Enregistrer", "Speichern", "Salva"],
    ["Delete", "Eliminar", "Excluir", "Supprimer", "Löschen", "Elimina"],
    ["Only letters and numbers, at least 2 characters.", "Sólo se acepta números y letras, al menos 2 caracteres.",
        "Use apenas letras e números, com pelo menos 2 caracteres.", "Lettres et chiffres uniquement, 2 caractères minimum.",
        "Nur Buchstaben und Ziffern, mindestens 2 Zeichen.", "Solo lettere e numeri, almeno 2 caratteri."],
    ["Choose an avatar and a username.", "Elige un avatar y un nombre de usuario.", "Escolha um avatar e um nome de usuário.",
        "Choisissez un avatar et un nom d’utilisateur.", "Wähle einen Avatar und einen Benutzernamen.", "Scegli un avatar e un nome utente."],
    ["Delete Profile", "Eliminar Perfil", "Excluir perfil", "Supprimer le profil", "Profil löschen", "Elimina profilo"],
    ["Are you sure you want to delete this profile?", "¿Estás seguro de que deseas eliminar este perfil?",
        "Tem certeza de que deseja excluir este perfil?", "Voulez-vous vraiment supprimer ce profil\u{a0}?",
        "Möchtest du dieses Profil wirklich löschen?", "Vuoi davvero eliminare questo profilo?"],
    ["Profile", "Perfil", "Perfil", "Profil", "Profil", "Profilo"],
    ["New profile", "Nuevo perfil", "Novo perfil", "Nouveau profil", "Neues Profil", "Nuovo profilo"],
    ["No name", "Sin nombre", "Sem nome", "Sans nom", "Ohne Namen", "Senza nome"],
    ["Saving…", "Guardando…", "Salvando…", "Enregistrement…", "Wird gespeichert…", "Salvataggio…"],
    ["Choose an avatar", "Elige un avatar", "Escolha um avatar", "Choisissez un avatar", "Wähle einen Avatar", "Scegli un avatar"],
    ["Loading…", "Cargando…", "Carregando…", "Chargement…", "Wird geladen…", "Caricamento…"],
    // subtitles turned off, as a preference
    ["None", "Desactivar", "Desativadas", "Désactivés", "Aus", "Disattivati"],
    // the on-screen keyboard
    ["space", "espacio", "espaço", "espace", "Leerzeichen", "spazio"],
    ["Shift", "Mayús", "Shift", "Maj", "Shift", "Shift"],
    // the top bar
    ["Home", "Inicio", "Início", "Accueil", "Startseite", "Home"],
    ["Simulcast", "Simulcast", "Simulcast", "Simulcast", "Simulcast", "Simulcast"],
    ["Series", "Series", "Séries", "Séries", "Serien", "Serie"],
    ["Movies", "Películas", "Filmes", "Films", "Filme", "Film"],
    ["Music", "Música", "Música", "Musique", "Musik", "Musica"],
    ["My List", "Mi Lista", "Minha lista", "Ma liste", "Meine Liste", "La mia lista"],
    ["Search", "Buscar", "Buscar", "Rechercher", "Suchen", "Cerca"],
    ["Settings", "Ajustes", "Configurações", "Paramètres", "Einstellungen", "Impostazioni"],
    ["Exit", "Salir", "Sair", "Quitter", "Beenden", "Esci"],
    ["AM", "a. m.", "AM", "AM", "AM", "AM"],
    ["PM", "p. m.", "PM", "PM", "PM", "PM"],
    // when a simulcast episode came out: the date, the hour and the minutes
    ["{}, {}:{} AM", "{}, {}:{} a. m", "{}, {}:{} AM", "{} {}:{} AM", "{}, {}:{} AM", "{}, {}:{} AM"],
    ["{}, {}:{} PM", "{}, {}:{} p. m", "{}, {}:{} PM", "{} {}:{} PM", "{}, {}:{} PM", "{}, {}:{} PM"],
    // home rows
    ["Continue watching", "Seguir viendo", "Continuar assistindo", "Continuer à regarder", "Weiterschauen", "Continua a guardare"],
    ["Watchlist", "Mi lista", "Minha lista", "Ma liste", "Meine Liste", "La mia lista"],
    ["Recommended for you", "Recomendado para ti", "Recomendado para você", "Recommandé pour vous", "Für dich empfohlen", "Consigliati per te"],
    ["Browse", "Explorar", "Explorar", "Explorer", "Entdecken", "Esplora"],
    ["Featured", "Destacados", "Destaques", "À la une", "Highlights", "In evidenza"],
    // facts about a show or an episode
    ["S{}", "T{}", "T{}", "S{}", "S{}", "S{}"],
    ["E{}", "E{}", "E{}", "É{}", "F{}", "E{}"],
    ["Movie", "Película", "Filme", "Film", "Film", "Film"],
    ["Artist", "Artista", "Artista", "Artiste", "Künstler", "Artista"],
    ["{} season", "{} temporada", "{} temporada", "{} saison", "{} Staffel", "{} stagione"],
    ["{} seasons", "{} temporadas", "{} temporadas", "{} saisons", "{} Staffeln", "{} stagioni"],
    ["{} episode", "{} episodio", "{} episódio", "{} épisode", "{} Folge", "{} episodio"],
    ["{} episodes", "{} episodios", "{} episódios", "{} épisodes", "{} Folgen", "{} episodi"],
    ["Sub | Dub", "Sub | Dob", "Leg | Dub", "VOST | VF", "OmU | Synchro", "Sub | Doppiato"],
    ["Sub", "Sub", "Leg", "VOST", "OmU", "Sub"],
    ["Dub", "Dob", "Dub", "VF", "Synchro", "Doppiato"],
    ["{} min", "{} min", "{} min", "{} min", "{} Min.", "{} min"],
    ["{} min left", "Quedan {} min", "Restam {} min", "Encore {} min", "Noch {} Min.", "Ancora {} min"],
    // the eyebrow over a title
    ["SERIES", "SERIES", "SÉRIE", "SÉRIE", "SERIE", "SERIE"],
    ["MOVIE", "PELÍCULA", "FILME", "FILM", "FILM", "FILM"],
    ["MUSIC", "MÚSICA", "MÚSICA", "MUSIQUE", "MUSIK", "MUSICA"],
    ["ARTIST", "ARTISTA", "ARTISTA", "ARTISTE", "KÜNSTLER", "ARTISTA"],
    ["EPISODE", "EPISODIO", "EPISÓDIO", "ÉPISODE", "FOLGE", "EPISODIO"],
    // lists and grids
    ["Empty", "Vacío", "Vazio", "Vide", "Leer", "Vuoto"],
    ["Couldn’t load", "No se pudo cargar", "Não foi possível carregar", "Impossible de charger", "Laden fehlgeschlagen", "Impossibile caricare"],
    ["Newest", "Reciente", "Recentes", "Récents", "Neueste", "Recenti"],
    ["Popular", "Popular", "Populares", "Populaires", "Beliebt", "Popolari"],
    ["Alphabetical", "Alfabético", "Alfabética", "Alphabétique", "Alphabetisch", "Alfabetico"],
    ["All", "Todo", "Todos", "Tous", "Alle", "Tutti"],
    ["Subtitled", "Subtitulado", "Legendado", "Sous-titré", "Untertitelt", "Sottotitolato"],
    ["Dubbed", "Doblado", "Dublado", "Doublé", "Synchronisiert", "Doppiato"],
    // a show's page
    ["Playback is coming in a future version.", "La reproducción llegará en una próxima versión.",
        "A reprodução chegará em uma versão futura.", "La lecture arrivera dans une prochaine version.",
        "Die Wiedergabe kommt in einer späteren Version.", "La riproduzione arriverà in una prossima versione."],
    ["Remove from My List", "Eliminar de mi lista", "Remover da minha lista", "Retirer de ma liste", "Von meiner Liste entfernen",
        "Rimuovi dalla mia lista"],
    ["Add to My List", "Agregar a mi lista", "Adicionar à minha lista", "Ajouter à ma liste", "Zu meiner Liste hinzufügen",
        "Aggiungi alla mia lista"],
    ["Audio and subtitles", "Audio y subtítulos", "Áudio e legendas", "Audio et sous-titres", "Audio und Untertitel", "Audio e sottotitoli"],
    ["Similar", "Similar", "Semelhantes", "Similaires", "Ähnliche Titel", "Simili"],
    ["Mark as watched", "Marcar como visto", "Marcar como assistido", "Marquer comme vu", "Als gesehen markieren", "Segna come visto"],
    ["Rate {}/5", "Calificar {}/5", "Avaliar {}/5", "Noter {}/5", "Mit {}/5 bewerten", "Valuta {}/5"],
    ["Watch", "Ver", "Assistir", "Regarder", "Ansehen", "Guarda"],
    ["Continue", "Continuar", "Continuar", "Reprendre", "Fortsetzen", "Continua"],
    ["Watch {}", "Ver {}", "Assistir {}", "Regarder {}", "{} ansehen", "Guarda {}"],
    ["Continue {}", "Continuar {}", "Continuar {}", "Reprendre {}", "{} fortsetzen", "Continua {}"],
    ["Season {}", "Temporada {}", "Temporada {}", "Saison {}", "Staffel {}", "Stagione {}"],
    ["SUBTITLES", "SUBTITULO", "LEGENDAS", "SOUS-TITRES", "UNTERTITEL", "SOTTOTITOLI"],
    ["AUDIO", "AUDIO", "ÁUDIO", "AUDIO", "AUDIO", "AUDIO"],
    ["Press Back to return.", "Pulsa Atrás para volver.", "Pressione Voltar para retornar.", "Appuyez sur Retour pour revenir.",
        "Drücke Zurück, um zurückzukehren.", "Premi Indietro per tornare."],
    ["Mark season as watched", "Marcar temporada como vista", "Marcar temporada como assistida", "Marquer la saison comme vue",
        "Staffel als gesehen markieren", "Segna la stagione come vista"],
    ["Newest first", "Más recientes primero", "Mais recentes primeiro", "Plus récents d’abord", "Neueste zuerst", "Più recenti prima"],
    ["Oldest first", "Más antiguos primero", "Mais antigos primeiro", "Plus anciens d’abord", "Älteste zuerst", "Più vecchi prima"],
    // an artist
    ["Videos", "Vídeos", "Vídeos", "Vidéos", "Videos", "Video"],
    ["Concerts", "Conciertos", "Shows", "Concerts", "Konzerte", "Concerti"],
    // notices
    ["Something went wrong", "Ocurrió un error", "Ocorreu um erro", "Une erreur s’est produite", "Ein Fehler ist aufgetreten",
        "Si è verificato un errore"],
    ["Marked as watched", "Marcado como visto", "Marcado como assistido", "Marqué comme vu", "Als gesehen markiert", "Segnato come visto"],
    ["Videos play on the TV", "Los videos se reproducen en la TV", "Os vídeos são reproduzidos na TV", "Les vidéos se lisent sur la TV",
        "Videos laufen auf dem Fernseher", "I video si riproducono sulla TV"],
    // settings
    ["Video quality", "Calidad de vídeo", "Qualidade do vídeo", "Qualité vidéo", "Videoqualität", "Qualità video"],
    ["Subtitles", "Subtítulos", "Legendas", "Sous-titres", "Untertitel", "Sottotitoli"],
    ["Previews", "Vistas previas", "Pré-visualizações", "Aperçus", "Vorschaubilder", "Anteprime"],
    ["Show “Skip intro”", "Mostrar «Saltar intro»", "Mostrar “Pular abertura”", "Afficher «\u{a0}Passer l’intro\u{a0}»",
        "„Intro überspringen“ anzeigen", "Mostra «Salta intro»"],
    ["Controls size", "Tamaño de los controles", "Tamanho dos controles", "Taille des commandes", "Größe der Steuerung",
        "Dimensione dei controlli"],
    ["First row", "Primera fila", "Primeira fileira", "Première rangée", "Erste Reihe", "Prima riga"],
    ["Interface", "Interfaz", "Interface", "Interface", "Oberfläche", "Interfaccia"],
    ["Clock", "Reloj", "Relógio", "Horloge", "Uhr", "Orologio"],
    ["Interface resolution", "Resolución de la interfaz", "Resolução da interface", "Résolution de l’interface",
        "Auflösung der Oberfläche", "Risoluzione dell’interfaccia"],
    ["Interface language", "Idioma de la interfaz", "Idioma da interface", "Langue de l’interface", "Sprache der Oberfläche",
        "Lingua dell’interfaccia"],
    ["Titles and synopses", "Títulos y sinopsis", "Títulos e sinopses", "Titres et résumés", "Titel und Beschreibungen", "Titoli e trame"],
    ["Preferred audio", "Audio preferido", "Áudio preferido", "Audio préféré", "Bevorzugtes Audio", "Audio preferito"],
    ["Preferred subtitles", "Subtítulos preferidos", "Legendas preferidas", "Sous-titres préférés", "Bevorzugte Untertitel",
        "Sottotitoli preferiti"],
    ["PLAYBACK", "REPRODUCCIÓN", "REPRODUÇÃO", "LECTURE", "WIEDERGABE", "RIPRODUZIONE"],
    ["HOME", "INICIO", "INÍCIO", "ACCUEIL", "STARTSEITE", "HOME"],
    ["DISPLAY", "PANTALLA", "TELA", "ÉCRAN", "BILDSCHIRM", "SCHERMO"],
    ["TOP BAR", "BARRA SUPERIOR", "BARRA SUPERIOR", "BARRE SUPÉRIEURE", "OBERE LEISTE", "BARRA SUPERIORE"],
    ["LANGUAGE", "IDIOMA", "IDIOMA", "LANGUE", "SPRACHE", "LINGUA"],
    ["ACCOUNT", "CUENTA", "CONTA", "COMPTE", "KONTO", "ACCOUNT"],
    ["Automatic", "Automático", "Automática", "Automatique", "Automatisch", "Automatica"],
    ["Burned into the video (Hardsub)", "Incrustados en el vídeo (Hardsub)", "Gravadas no vídeo (Hardsub)",
        "Incrustés dans la vidéo (Hardsub)", "Ins Video eingebrannt (Hardsub)", "Impressi nel video (Hardsub)"],
    ["Drawn by the app (Softsub)", "Dibujados por la app (Softsub)", "Desenhadas pelo app (Softsub)", "Affichés par l’app (Softsub)",
        "Von der App gezeichnet (Softsub)", "Disegnati dall’app (Softsub)"],
    ["Yes", "Sí", "Sim", "Oui", "Ja", "Sì"],
    ["No", "No", "Não", "Non", "Nein", "No"],
    ["{} seconds", "{} segundos", "{} segundos", "{} secondes", "{} Sekunden", "{} secondi"],
    ["Suggestions", "Sugerencias", "Sugestões", "Suggestions", "Vorschläge", "Suggerimenti"],
    ["Full", "Completo", "Completa", "Complète", "Vollständig", "Completa"],
    ["Lite", "Ligero", "Leve", "Légère", "Schlank", "Leggera"],
    ["Normal", "Normal", "Normal", "Normale", "Normal", "Normale"],
    ["Small", "Pequeño", "Pequeno", "Petite", "Klein", "Piccola"],
    ["24-hour", "24 horas", "24 horas", "24 heures", "24 Stunden", "24 ore"],
    ["12-hour (AM / PM)", "12 horas (a. m. / p. m.)", "12 horas (AM / PM)", "12 heures (AM / PM)", "12 Stunden (AM / PM)", "12 ore (AM / PM)"],
    ["1080p (recommended)", "1080p (recomendada)", "1080p (recomendada)", "1080p (recommandée)", "1080p (empfohlen)", "1080p (consigliata)"],
    ["Show", "Mostrar", "Mostrar", "Afficher", "Anzeigen", "Mostra"],
    ["Hide", "Ocultar", "Ocultar", "Masquer", "Ausblenden", "Nascondi"],
    ["TV language ({})", "Idioma de la TV ({})", "Idioma da TV ({})", "Langue de la TV ({})", "TV-Sprache ({})", "Lingua della TV ({})"],
    // the player's buttons, menus and notices
    ["Info", "Información", "Informações", "Infos", "Infos", "Info"],
    ["Dislike", "No me gusta", "Não gostei", "Je n’aime pas", "Gefällt mir nicht", "Non mi piace"],
    ["Like", "Me gusta", "Gostei", "J’aime", "Gefällt mir", "Mi piace"],
    ["Previous episode", "Episodio anterior", "Episódio anterior", "Épisode précédent", "Vorherige Folge", "Episodio precedente"],
    ["Play", "Reproducir", "Reproduzir", "Lecture", "Abspielen", "Riproduci"],
    ["Pause", "Pausar", "Pausar", "Pause", "Pause", "Pausa"],
    // the button's name; "Next Episode" (capitals) is the button over the credits
    ["Next episode", "Episodio siguiente", "Próximo episódio", "Épisode suivant", "Nächste Folge", "Episodio successivo"],
    ["Audio", "Audio", "Áudio", "Audio", "Audio", "Audio"],
    ["Quality", "Calidad", "Qualidade", "Qualité", "Qualität", "Qualità"],
    ["Auto", "Automática", "Automática", "Automatique", "Automatisch", "Automatica"],
    // subtitles turned off, in the player's menu
    ["Off", "Desactivados", "Desativadas", "Désactivés", "Aus", "Disattivati"],
    ["There’s no next episode", "No hay un episodio siguiente", "Não há próximo episódio", "Il n’y a pas d’épisode suivant",
        "Es gibt keine nächste Folge", "Non c’è un episodio successivo"],
    ["There’s no previous episode", "No hay un episodio anterior", "Não há episódio anterior", "Il n’y a pas d’épisode précédent",
        "Es gibt keine vorherige Folge", "Non c’è un episodio precedente"],
    ["Next Episode", "Siguiente episodio", "Próximo episódio", "Épisode suivant", "Nächste Folge", "Prossimo episodio"],
    ["Finish", "Terminar", "Encerrar", "Terminer", "Beenden", "Termina"],
    ["Skip recap", "Saltar resumen", "Pular recapitulação", "Passer le résumé", "Rückblick überspringen", "Salta riepilogo"],
    ["Skip intro", "Saltar intro", "Pular abertura", "Passer l’intro", "Intro überspringen", "Salta intro"],
    ["Skip credits", "Saltar créditos", "Pular créditos", "Passer le générique", "Abspann überspringen", "Salta i titoli di coda"],
    ["Skip preview", "Saltar avance", "Pular prévia", "Passer l’aperçu", "Vorschau überspringen", "Salta anteprima"],
    ["Playback stopped", "Se detuvo la reproducción", "A reprodução parou", "La lecture s’est arrêtée", "Die Wiedergabe wurde angehalten",
        "La riproduzione si è interrotta"],
    ["OK to resume from {} · Back to return", "OK para seguir desde {} · Atrás para volver", "OK para continuar de {} · Voltar para retornar",
        "OK pour reprendre à {} · Retour pour revenir", "OK zum Fortsetzen ab {} · Zurück zum Verlassen",
        "OK per riprendere da {} · Indietro per tornare"],
    ["Couldn’t play this", "No se pudo reproducir", "Não foi possível reproduzir", "Lecture impossible", "Wiedergabe nicht möglich",
        "Impossibile riprodurre"],
    ["Back to return", "Atrás para volver", "Voltar para retornar", "Retour pour revenir", "Zurück zum Verlassen", "Indietro per tornare"],
    ["Resuming from {}", "Continuando desde {}", "Continuando de {}", "Reprise à {}", "Fortsetzen ab {}", "Ripresa da {}"],
    ["Episode {}", "Episodio {}", "Episódio {}", "Épisode {}", "Folge {}", "Episodio {}"],
    // languages, as the player's menus name them
    ["English (India)", "Inglés (India)", "Inglês (Índia)", "Anglais (Inde)", "Englisch (Indien)", "Inglese (India)"],
    ["Spanish (Latin America)", "Español (América Latina)", "Espanhol (América Latina)", "Espagnol (Amérique latine)",
        "Spanisch (Lateinamerika)", "Spagnolo (America Latina)"],
    ["Spanish (Spain)", "Español (España)", "Espanhol (Espanha)", "Espagnol (Espagne)", "Spanisch (Spanien)", "Spagnolo (Spagna)"],
    ["Portuguese (Brazil)", "Portugués (Brasil)", "Português (Brasil)", "Portugais (Brésil)", "Portugiesisch (Brasilien)",
        "Portoghese (Brasile)"],
    ["Portuguese (Portugal)", "Portugués (Portugal)", "Português (Portugal)", "Portugais (Portugal)", "Portugiesisch (Portugal)",
        "Portoghese (Portogallo)"],
    ["Chinese (Mandarin)", "Chino (Mandarín)", "Chinês (Mandarim)", "Chinois (mandarin)", "Chinesisch (Mandarin)", "Cinese (mandarino)"],
    // languages
    ["Arabic", "Árabe", "Árabe", "Arabe", "Arabisch", "Arabo"],
    ["Bengali", "Bengalí", "Bengali", "Bengali", "Bengalisch", "Bengalese"],
    ["Catalan", "Catalán", "Catalão", "Catalan", "Katalanisch", "Catalano"],
    ["Chinese", "Chino", "Chinês", "Chinois", "Chinesisch", "Cinese"],
    ["Czech", "Checo", "Tcheco", "Tchèque", "Tschechisch", "Ceco"],
    ["Dutch", "Neerlandés", "Holandês", "Néerlandais", "Niederländisch", "Olandese"],
    ["English", "Inglés", "Inglês", "Anglais", "Englisch", "Inglese"],
    ["French", "Francés", "Francês", "Français", "Französisch", "Francese"],
    ["German", "Alemán", "Alemão", "Allemand", "Deutsch", "Tedesco"],
    ["Greek", "Griego", "Grego", "Grec", "Griechisch", "Greco"],
    ["Hebrew", "Hebreo", "Hebraico", "Hébreu", "Hebräisch", "Ebraico"],
    ["Hindi", "Hindi", "Hindi", "Hindi", "Hindi", "Hindi"],
    ["Hungarian", "Húngaro", "Húngaro", "Hongrois", "Ungarisch", "Ungherese"],
    ["Indonesian", "Indonesio", "Indonésio", "Indonésien", "Indonesisch", "Indonesiano"],
    ["Italian", "Italiano", "Italiano", "Italien", "Italienisch", "Italiano"],
    ["Japanese", "Japonés", "Japonês", "Japonais", "Japanisch", "Giapponese"],
    ["Kannada", "Canarés", "Canarim", "Kannada", "Kannada", "Kannada"],
    ["Korean", "Coreano", "Coreano", "Coréen", "Koreanisch", "Coreano"],
    ["Malay", "Malayo", "Malaio", "Malais", "Malaiisch", "Malese"],
    ["Malayalam", "Malayalam", "Malaiala", "Malayalam", "Malayalam", "Malayalam"],
    ["Marathi", "Maratí", "Marati", "Marathi", "Marathi", "Marathi"],
    ["Polish", "Polaco", "Polonês", "Polonais", "Polnisch", "Polacco"],
    ["Portuguese", "Portugués", "Português", "Portugais", "Portugiesisch", "Portoghese"],
    ["Romanian", "Rumano", "Romeno", "Roumain", "Rumänisch", "Rumeno"],
    ["Russian", "Ruso", "Russo", "Russe", "Russisch", "Russo"],
    ["Spanish", "Español", "Espanhol", "Espagnol", "Spanisch", "Spagnolo"],
    ["Swedish", "Sueco", "Sueco", "Suédois", "Schwedisch", "Svedese"],
    ["Tamil", "Tamil", "Tâmil", "Tamoul", "Tamil", "Tamil"],
    ["Telugu", "Telugu", "Télugo", "Télougou", "Telugu", "Telugu"],
    ["Thai", "Tailandés", "Tailandês", "Thaï", "Thailändisch", "Thailandese"],
    ["Turkish", "Turco", "Turco", "Turc", "Türkisch", "Turco"],
    ["Ukrainian", "Ucraniano", "Ucraniano", "Ukrainien", "Ukrainisch", "Ucraino"],
    ["Vietnamese", "Vietnamita", "Vietnamita", "Vietnamien", "Vietnamesisch", "Vietnamita"],
    // where a language is spoken
    ["Brazil", "Brasil", "Brasil", "Brésil", "Brasilien", "Brasile"],
    ["China", "China", "China", "Chine", "China", "Cina"],
    ["France", "Francia", "França", "France", "Frankreich", "Francia"],
    ["Germany", "Alemania", "Alemanha", "Allemagne", "Deutschland", "Germania"],
    ["Hong Kong", "Hong Kong", "Hong Kong", "Hong Kong", "Hongkong", "Hong Kong"],
    ["India", "India", "Índia", "Inde", "Indien", "India"],
    ["Indonesia", "Indonesia", "Indonésia", "Indonésie", "Indonesien", "Indonesia"],
    ["Italy", "Italia", "Itália", "Italie", "Italien", "Italia"],
    ["Japan", "Japón", "Japão", "Japon", "Japan", "Giappone"],
    ["Latin America", "Latinoamérica", "América Latina", "Amérique latine", "Lateinamerika", "America Latina"],
    ["Malaysia", "Malasia", "Malásia", "Malaisie", "Malaysia", "Malesia"],
    ["Mexico", "México", "México", "Mexique", "Mexiko", "Messico"],
    ["Poland", "Polonia", "Polônia", "Pologne", "Polen", "Polonia"],
    ["Portugal", "Portugal", "Portugal", "Portugal", "Portugal", "Portogallo"],
    ["Russia", "Rusia", "Rússia", "Russie", "Russland", "Russia"],
    ["Saudi Arabia", "Arabia Saudita", "Arábia Saudita", "Arabie saoudite", "Saudi-Arabien", "Arabia Saudita"],
    ["South Korea", "Corea del Sur", "Coreia do Sul", "Corée du Sud", "Südkorea", "Corea del Sud"],
    ["Spain", "España", "Espanha", "Espagne", "Spanien", "Spagna"],
    ["Taiwan", "Taiwán", "Taiwan", "Taïwan", "Taiwan", "Taiwan"],
    ["Thailand", "Tailandia", "Tailândia", "Thaïlande", "Thailand", "Thailandia"],
    ["Turkey", "Turquía", "Turquia", "Turquie", "Türkei", "Turchia"],
    ["United Kingdom", "Reino Unido", "Reino Unido", "Royaume-Uni", "Vereinigtes Königreich", "Regno Unito"],
    ["United States", "Estados Unidos", "Estados Unidos", "États-Unis", "USA", "Stati Uniti"],
    ["Vietnam", "Vietnam", "Vietnã", "Viêt Nam", "Vietnam", "Vietnam"],
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;

    use super::*;

    /// Every Rust file under `dir` but this one, with its path.
    fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("the sources").flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") && !path.ends_with("app/i18n.rs") {
                out.push((path.display().to_string(), std::fs::read_to_string(&path).expect("a source")));
            }
        }
    }

    /// The Rust string literal `rest` starts inside of, up to its closing quote.
    fn literal(rest: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    '\'' => out.push('\''),
                    'u' => {
                        let hex: String = chars.by_ref().take_while(|c| *c != '}').collect();
                        out.push(char::from_u32(u32::from_str_radix(hex.trim_start_matches('{'), 16).ok()?)?);
                    }
                    _ => return None,
                },
                c => out.push(c),
            }
        }
        None
    }

    /// The keys a source uses: the literal after each `t(`, `tr!(` and `Text::Key(` (`None` for one that can't be
    /// read).
    fn keys_in(source: &str) -> Vec<Option<String>> {
        let mut keys = Vec::new();
        for marker in ["t(\"", "tr!(\"", "Text::Key(\""] {
            let mut from = 0;
            while let Some(at) = source[from..].find(marker) {
                let start = from + at;
                from = start + marker.len();
                // the call itself, not the end of another's name (`print(`, `include_str!(`)
                if source[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                keys.push(literal(&source[from..]));
            }
        }
        keys
    }

    #[test]
    fn every_key_has_its_translations() {
        let mut files = Vec::new();
        sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut problems = Vec::new();
        let mut used = HashSet::new();
        for (file, source) in &files {
            for key in keys_in(source) {
                let Some(key) = key else {
                    problems.push(format!("{file}: a key that isn't a plain string"));
                    continue;
                };
                let Some(row) = TABLE.get(key.as_str()) else {
                    problems.push(format!("{file}: {key:?} isn't in the table"));
                    continue;
                };
                used.insert(key.clone());
                for (lang, text) in Lang::ALL.iter().zip(row.iter()).skip(1) {
                    if text.trim().is_empty() {
                        problems.push(format!("{key:?} has no {lang:?}"));
                    } else if text.matches("{}").count() != key.matches("{}").count() {
                        problems.push(format!("{key:?} in {lang:?} has other placeholders: {text:?}"));
                    }
                }
            }
        }
        for row in ENTRIES {
            if !used.contains(row[0]) {
                problems.push(format!("{:?} is in the table, but nothing uses it", row[0]));
            }
        }
        assert_eq!(TABLE.len(), ENTRIES.len(), "a key is in the table twice");
        assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    }

    #[test]
    fn the_scan_finds_keys_and_only_keys() {
        let source = "ui.gfx.text(font, 1.0, x, y, t(\"Home\"), c); tr!(\"Season {}\", 2); print(\"no\"); include_str!(\"no\"); \
                      Text::Key(\"a \\\"b\\\"\")";
        let keys: Vec<String> = keys_in(source).into_iter().flatten().collect();
        assert_eq!(keys, ["Home", "Season {}", "a \"b\""]);
    }

    #[test]
    fn placeholders_take_the_values_in_order() {
        assert_eq!(fill("{} · {}", &[&"T1", &5]), "T1 · 5");
        assert_eq!(fill("Rate {}/5", &[&3]), "Rate 3/5");
        assert_eq!(fill("no values", &[]), "no values");
        assert_eq!(translate(Lang::Es, "Season {}"), "Temporada {}");
        assert_eq!(translate(Lang::De, "not a key"), "not a key");
        assert_eq!(translate(Lang::En, "Season {}"), "Season {}");
    }

    #[test]
    fn locales_name_their_language() {
        assert_eq!(Lang::from_code("en-US"), Some(Lang::En));
        assert_eq!(Lang::from_code("es-419"), Some(Lang::Es));
        assert_eq!(Lang::from_code("pt_BR"), Some(Lang::Pt));
        assert_eq!(Lang::from_code("FR"), Some(Lang::Fr));
        assert_eq!(Lang::from_code("de-AT"), Some(Lang::De));
        assert_eq!(Lang::from_code("it"), Some(Lang::It));
        assert_eq!(Lang::from_code("ko-KR"), None);
        assert_eq!(Lang::from_code("auto"), None);
        assert_eq!(Lang::from_code(""), None);
        for lang in Lang::ALL {
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
            assert_eq!(Lang::from_code(lang.locale()), Some(lang));
        }
    }

    #[test]
    fn french_counts_none_as_one() {
        assert!(singular_in(Lang::Fr, 0) && singular_in(Lang::Fr, 1) && !singular_in(Lang::Fr, 2));
        assert!(!singular_in(Lang::Es, 0) && singular_in(Lang::Es, 1) && !singular_in(Lang::En, 2));
    }

    #[test]
    fn dates_go_the_language_s_way() {
        assert_eq!(short_date_in(Lang::Es, 2025, 10, 4), "04/10/25");
        assert_eq!(short_date_in(Lang::En, 2025, 10, 4), "10/04/25");
        assert_eq!(short_date_in(Lang::De, 2025, 10, 4), "04.10.25");
    }
}
