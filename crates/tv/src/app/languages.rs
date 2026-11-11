//! Language names in Spanish, "Idioma (Región)", for the codes Crunchyroll uses ("es-419", "ja-JP").

pub fn language_name(code: &str) -> String {
    if code.is_empty() {
        return String::new();
    }
    if code == "off" {
        return "Desactivar".into();
    }
    let mut parts = code.split('-');
    let language = parts.next().unwrap_or("");
    let region = parts.next().unwrap_or("");
    let name = match language {
        "ar" => "Árabe",
        "ca" => "Catalán",
        "de" => "Alemán",
        "en" => "Inglés",
        "es" => "Español",
        "fr" => "Francés",
        "hi" => "Hindi",
        "id" => "Indonesio",
        "it" => "Italiano",
        "ja" => "Japonés",
        "ko" => "Coreano",
        "ms" => "Malayo",
        "pl" => "Polaco",
        "pt" => "Portugués",
        "ru" => "Ruso",
        "ta" => "Tamil",
        "te" => "Telugu",
        "th" => "Tailandés",
        "tr" => "Turco",
        "vi" => "Vietnamita",
        "zh" => "Chino",
        "kn" => "Canarés",
        "ml" => "Malayalam",
        "bn" => "Bengalí",
        "mr" => "Maratí",
        "nl" => "Neerlandés",
        "sv" => "Sueco",
        "uk" => "Ucraniano",
        "he" => "Hebreo",
        "el" => "Griego",
        "cs" => "Checo",
        "hu" => "Húngaro",
        "ro" => "Rumano",
        _ => return code.to_string(),
    };
    let region_name = match region {
        "" => return name.to_string(),
        "419" => "Latinoamérica",
        "SA" => "Arabia Saudita",
        "ES" => "España",
        "US" => "Estados Unidos",
        "GB" => "Reino Unido",
        "IN" => "India",
        "BR" => "Brasil",
        "PT" => "Portugal",
        "FR" => "Francia",
        "DE" => "Alemania",
        "IT" => "Italia",
        "JP" => "Japón",
        "KR" => "Corea del Sur",
        "RU" => "Rusia",
        "ID" => "Indonesia",
        "MY" => "Malasia",
        "PL" => "Polonia",
        "TH" => "Tailandia",
        "TR" => "Turquía",
        "VN" => "Vietnam",
        "CN" => "China",
        "TW" => "Taiwán",
        "HK" => "Hong Kong",
        "MX" => "México",
        other => other,
    };
    format!("{name} ({region_name})")
}
