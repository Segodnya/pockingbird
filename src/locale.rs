//! Derive a locale id from a `.po` path.
//!
//! The standard gettext layout is `.../<locale>/LC_MESSAGES/<domain>.po`, so the
//! component immediately before `LC_MESSAGES` is the locale id. When the path
//! does not follow this layout, fall back to the full path as the id — the
//! report shows whichever id was used, and a unique path keeps columns distinct.

use std::ffi::OsStr;
use std::path::Path;

const LC_MESSAGES: &str = "LC_MESSAGES";

/// Locale id for a `.po` path (standard layout → `<locale>`, else path string).
pub fn locale_id(path: &Path) -> String {
    let components: Vec<&OsStr> = path.components().map(|c| c.as_os_str()).collect();
    for (index, component) in components.iter().enumerate() {
        if *component == OsStr::new(LC_MESSAGES) && index > 0 {
            return components[index - 1].to_string_lossy().into_owned();
        }
    }
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_layout_yields_locale() {
        assert_eq!(
            locale_id(Path::new("locales/ru/LC_MESSAGES/messages.po")),
            "ru"
        );
        assert_eq!(
            locale_id(Path::new("/app/i18n/pt_BR/LC_MESSAGES/django.po")),
            "pt_BR"
        );
    }

    #[test]
    fn non_standard_layout_falls_back_to_path() {
        let path = "weird/place/foo.po";
        assert_eq!(locale_id(Path::new(path)), path);
    }
}
