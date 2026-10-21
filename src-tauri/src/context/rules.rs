use crate::core::types::AppCategory;
use serde::{Deserialize, Serialize};
use std::sync::{OnceLock, RwLock};
use tauri::Manager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryRule {
    /// Basename exato, sem distinguir maiúsculas. None = qualquer executável.
    pub exe: Option<String>,
    /// Host exato ou subdomínio; nunca substring livre. None = qualquer URL.
    pub domain: Option<String>,
    pub category: AppCategory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextRules {
    /// Limite de caracteres por trecho (antes/seleção/depois), máximo 2048.
    pub max_chars: usize,
    /// Primeira regra que corresponde vence.
    pub categories: Vec<CategoryRule>,
}

impl Default for ContextRules {
    fn default() -> Self {
        let mut categories = Vec::new();
        for (domain, category) in [
            ("mail.google.com", AppCategory::Email),
            ("outlook.live.com", AppCategory::Email),
            ("outlook.office.com", AppCategory::Email),
            ("slack.com", AppCategory::Work),
            ("discord.com", AppCategory::Personal),
        ] {
            categories.push(CategoryRule {
                exe: None,
                domain: Some(domain.into()),
                category,
            });
        }
        for (exe, category) in [
            ("outlook.exe", AppCategory::Email),
            ("thunderbird.exe", AppCategory::Email),
            ("code.exe", AppCategory::Work),
            ("slack.exe", AppCategory::Work),
            ("ms-teams.exe", AppCategory::Work),
            ("discord.exe", AppCategory::Personal),
            ("whatsapp.exe", AppCategory::Personal),
        ] {
            categories.push(CategoryRule {
                exe: Some(exe.into()),
                domain: None,
                category,
            });
        }
        Self {
            max_chars: 512,
            categories,
        }
    }
}

fn current() -> &'static RwLock<ContextRules> {
    static RULES: OnceLock<RwLock<ContextRules>> = OnceLock::new();
    RULES.get_or_init(|| RwLock::new(ContextRules::default()))
}

pub fn get() -> ContextRules {
    current().read().map(|r| r.clone()).unwrap_or_default()
}

impl ContextRules {
    fn validate(&self) -> Result<(), String> {
        if self.max_chars > 2048 || self.categories.len() > 256 {
            return Err("máximo: 2048 caracteres e 256 regras".into());
        }
        for rule in &self.categories {
            if rule
                .exe
                .as_ref()
                .is_some_and(|e| e.is_empty() || e.len() > 128 || e.contains(['/', '\\']))
                || rule.domain.as_ref().is_some_and(|d| {
                    d.is_empty() || d.len() > 253 || d.contains(['/', ':', '@', ' '])
                })
            {
                return Err(
                    "regra inválida: use basename do executável e hostname sem protocolo".into(),
                );
            }
        }
        Ok(())
    }

    pub fn classify(&self, exe: &str, url: Option<&str>) -> AppCategory {
        let host = url.and_then(|s| reqwest::Url::parse(s).ok()).and_then(|u| {
            u.host_str()
                .map(|s| s.trim_end_matches('.').to_ascii_lowercase())
        });
        self.categories
            .iter()
            .find(|r| {
                r.exe.as_ref().is_none_or(|e| e.eq_ignore_ascii_case(exe))
                    && r.domain.as_ref().is_none_or(|d| {
                        host.as_ref().is_some_and(|h| {
                            let d = d.trim_end_matches('.').to_ascii_lowercase();
                            h == &d || h.ends_with(&format!(".{d}"))
                        })
                    })
            })
            .map(|r| r.category)
            .unwrap_or_default()
    }
}

/// Lê o JSON das regras; aceita BOM UTF-8 (Bloco de Notas/PowerShell gravam com ele).
fn parse(bytes: &[u8]) -> Option<ContextRules> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    serde_json::from_slice::<ContextRules>(bytes)
        .ok()
        .filter(|rules| rules.validate().is_ok())
}

pub fn init(app: &tauri::AppHandle) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let path = dir.join("context-rules.json");
    match std::fs::read(&path) {
        Ok(bytes) => match parse(&bytes) {
            Some(rules) => {
                if let Ok(mut state) = current().write() {
                    *state = rules;
                }
            }
            _ => tracing::warn!("context-rules.json inválido; usando regras padrão"),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = save(app, ContextRules::default());
        }
        Err(_) => tracing::warn!("context-rules.json indisponível; usando regras padrão"),
    }
}

pub fn save(app: &tauri::AppHandle, rules: ContextRules) -> Result<(), String> {
    rules.validate()?;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut state = current().write().map_err(|_| "regras indisponíveis")?;
    let temp = dir.join("context-rules.json.tmp");
    std::fs::write(
        &temp,
        serde_json::to_vec_pretty(&rules).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temp, dir.join("context-rules.json")).map_err(|e| e.to_string())?;
    *state = rules;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rules_file_with_utf8_bom_is_accepted() {
        let json = serde_json::to_vec(&ContextRules::default()).unwrap();
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(&json);
        let parsed = parse(&with_bom).expect("BOM aceito");
        assert_eq!(serde_json::to_vec(&parsed).unwrap(), json);
        assert!(parse(&[0xEF, 0xBB, 0xBF, b'{']).is_none());
    }
    #[test]
    fn categories_match_host_boundaries_and_order() {
        let rules = ContextRules::default();
        assert_eq!(
            rules.classify("CHROME.EXE", Some("https://mail.google.com/a")),
            AppCategory::Email
        );
        assert_eq!(
            rules.classify("chrome.exe", Some("https://mail.google.com.evil.org/")),
            AppCategory::Other
        );
        assert_eq!(
            rules.classify("chrome.exe", Some("https://evil.org/mail.google.com")),
            AppCategory::Other
        );
        assert_eq!(rules.classify("CODE.EXE", None), AppCategory::Work);
        assert_eq!(rules.classify("xcode.exe", None), AppCategory::Other);
    }
}
