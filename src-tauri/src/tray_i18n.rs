//! Textos do tray (menu do ícone) nos dois idiomas da interface. O tray vive no Rust, fora da
//! UI React, então tem a própria tabelinha; a regra do idioma é a mesma do `ui_language`:
//! "pt-BR" ou "en" valem; "auto" (e qualquer outro valor) segue o idioma do Windows.

use tauri::menu::{CheckMenuItem, MenuItem};
use tauri::{AppHandle, Manager, Wry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Pt,
    En,
}

/// Rótulo do item de atualização, que muda com o andamento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateLabel {
    /// Ainda não procurou (ou procurou e falhou em silêncio).
    Check,
    UpToDate,
    /// Atualização pronta para instalar: `x.y.z`.
    Install(String),
    Downloading,
    Failed,
}

/// Os itens do menu que trocam de texto com o idioma.
pub struct TrayItems {
    pub open: MenuItem<Wry>,
    pub autostart: CheckMenuItem<Wry>,
    pub quit: MenuItem<Wry>,
}

/// Idioma efetivo para um valor de `ui_language`.
pub fn resolve(setting: &str, system: Lang) -> Lang {
    match setting {
        "pt-BR" => Lang::Pt,
        "en" => Lang::En,
        _ => system,
    }
}

/// Idioma da interface do Windows (português ou, para o resto, inglês).
pub fn system_lang() -> Lang {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetUserDefaultUILanguage() -> u16;
        }
        // LANGID: os 10 bits baixos são o idioma primário (0x16 = português).
        // SAFETY: função do Win32 sem argumentos nem efeitos colaterais.
        let id = unsafe { GetUserDefaultUILanguage() };
        if id & 0x3FF == 0x16 {
            return Lang::Pt;
        }
    }
    Lang::En
}

/// Idioma atual conforme o Settings.
pub fn current(app: &AppHandle) -> Lang {
    let setting = app
        .try_state::<crate::settings::SettingsStore>()
        .map(|s| s.get().ui_language)
        .unwrap_or_default();
    resolve(&setting, system_lang())
}

pub fn open(lang: Lang) -> &'static str {
    match lang {
        Lang::Pt => "Abrir Vyrp",
        Lang::En => "Open Vyrp",
    }
}

pub fn autostart(lang: Lang) -> &'static str {
    match lang {
        Lang::Pt => "Abrir ao iniciar o Windows",
        Lang::En => "Open when Windows starts",
    }
}

pub fn quit(lang: Lang) -> &'static str {
    match lang {
        Lang::Pt => "Sair",
        Lang::En => "Quit",
    }
}

pub fn update(lang: Lang, label: &UpdateLabel) -> String {
    match (lang, label) {
        (Lang::Pt, UpdateLabel::Check) => "Procurar atualizações".into(),
        (Lang::En, UpdateLabel::Check) => "Check for updates".into(),
        (Lang::Pt, UpdateLabel::UpToDate) => "Procurar atualizações (em dia)".into(),
        (Lang::En, UpdateLabel::UpToDate) => "Check for updates (up to date)".into(),
        (Lang::Pt, UpdateLabel::Install(v)) => format!("Instalar atualização {v}"),
        (Lang::En, UpdateLabel::Install(v)) => format!("Install update {v}"),
        (Lang::Pt, UpdateLabel::Downloading) => "Baixando atualização…".into(),
        (Lang::En, UpdateLabel::Downloading) => "Downloading update…".into(),
        (Lang::Pt, UpdateLabel::Failed) => "Atualização falhou: tentar de novo".into(),
        (Lang::En, UpdateLabel::Failed) => "Update failed: try again".into(),
    }
}

/// Troca o texto de todos os itens (chamado quando o `ui_language` muda).
pub fn apply(app: &AppHandle) {
    let lang = current(app);
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.open.set_text(open(lang));
        let _ = items.autostart.set_text(autostart(lang));
        let _ = items.quit.set_text(quit(lang));
    }
    if let Some(updates) = app.try_state::<crate::updates::Updates>() {
        updates.relabel(lang);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_language_wins_and_auto_follows_the_system() {
        assert_eq!(resolve("en", Lang::Pt), Lang::En);
        assert_eq!(resolve("pt-BR", Lang::En), Lang::Pt);
        assert_eq!(resolve("auto", Lang::Pt), Lang::Pt);
        assert_eq!(resolve("auto", Lang::En), Lang::En);
        assert_eq!(resolve("klingon", Lang::En), Lang::En);
        assert_eq!(resolve("", Lang::Pt), Lang::Pt);
    }

    #[test]
    fn every_label_exists_in_both_languages_and_differs() {
        for (pt, en) in [
            (open(Lang::Pt), open(Lang::En)),
            (autostart(Lang::Pt), autostart(Lang::En)),
            (quit(Lang::Pt), quit(Lang::En)),
        ] {
            assert!(!pt.is_empty() && !en.is_empty());
            assert_ne!(pt, en);
        }
        for label in [
            UpdateLabel::Check,
            UpdateLabel::UpToDate,
            UpdateLabel::Install("1.2.3".into()),
            UpdateLabel::Downloading,
            UpdateLabel::Failed,
        ] {
            let (pt, en) = (update(Lang::Pt, &label), update(Lang::En, &label));
            assert!(!pt.is_empty() && !en.is_empty());
            assert_ne!(pt, en, "{label:?}");
        }
    }

    #[test]
    fn the_install_label_carries_the_version() {
        let label = UpdateLabel::Install("0.4.2".into());
        assert_eq!(update(Lang::Pt, &label), "Instalar atualização 0.4.2");
        assert_eq!(update(Lang::En, &label), "Install update 0.4.2");
    }

    #[test]
    fn system_language_is_one_of_the_two() {
        assert!(matches!(system_lang(), Lang::Pt | Lang::En));
    }
}
