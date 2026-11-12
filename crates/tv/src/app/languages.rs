//! Language names in the interface's language, "Language (Region)", for the codes Crunchyroll uses ("es-419",
//! "ja-JP"). Never a language's own name for itself: the font has Latin letters only.

use super::i18n::t;

pub fn language_name(code: &str) -> String {
    if code.is_empty() {
        return String::new();
    }
    if code == "off" {
        return t("None").into();
    }
    let mut parts = code.split('-');
    let language = parts.next().unwrap_or("");
    let region = parts.next().unwrap_or("");
    let name = match language {
        "ar" => t("Arabic"),
        "ca" => t("Catalan"),
        "de" => t("German"),
        "en" => t("English"),
        "es" => t("Spanish"),
        "fr" => t("French"),
        "hi" => t("Hindi"),
        "id" => t("Indonesian"),
        "it" => t("Italian"),
        "ja" => t("Japanese"),
        "ko" => t("Korean"),
        "ms" => t("Malay"),
        "pl" => t("Polish"),
        "pt" => t("Portuguese"),
        "ru" => t("Russian"),
        "ta" => t("Tamil"),
        "te" => t("Telugu"),
        "th" => t("Thai"),
        "tr" => t("Turkish"),
        "vi" => t("Vietnamese"),
        "zh" => t("Chinese"),
        "kn" => t("Kannada"),
        "ml" => t("Malayalam"),
        "bn" => t("Bengali"),
        "mr" => t("Marathi"),
        "nl" => t("Dutch"),
        "sv" => t("Swedish"),
        "uk" => t("Ukrainian"),
        "he" => t("Hebrew"),
        "el" => t("Greek"),
        "cs" => t("Czech"),
        "hu" => t("Hungarian"),
        "ro" => t("Romanian"),
        _ => return code.to_string(),
    };
    let region_name = match region {
        "" => return name.to_string(),
        "419" => t("Latin America"),
        "SA" => t("Saudi Arabia"),
        "ES" => t("Spain"),
        "US" => t("United States"),
        "GB" => t("United Kingdom"),
        "IN" => t("India"),
        "BR" => t("Brazil"),
        "PT" => t("Portugal"),
        "FR" => t("France"),
        "DE" => t("Germany"),
        "IT" => t("Italy"),
        "JP" => t("Japan"),
        "KR" => t("South Korea"),
        "RU" => t("Russia"),
        "ID" => t("Indonesia"),
        "MY" => t("Malaysia"),
        "PL" => t("Poland"),
        "TH" => t("Thailand"),
        "TR" => t("Turkey"),
        "VN" => t("Vietnam"),
        "CN" => t("China"),
        "TW" => t("Taiwan"),
        "HK" => t("Hong Kong"),
        "MX" => t("Mexico"),
        other => other,
    };
    format!("{name} ({region_name})")
}
