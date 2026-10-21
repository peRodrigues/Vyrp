//! Atualização (V23): tauri-plugin-updater lendo o `latest.json` do GitHub Releases
//! (assinado; a chave pública está no `tauri.conf.json`). O app só PROCURA sozinho; baixar
//! e instalar é sempre um clique do usuário no tray.

use std::sync::Mutex;
use std::time::Duration;

use tauri::menu::MenuItem;
use tauri::{AppHandle, Emitter, Manager, Wry};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::tray_i18n::{self, Lang, UpdateLabel};

/// Evento para o hub: `{ "version": "x.y.z" }` quando há atualização.
pub const EVENT_AVAILABLE: &str = "update://available";
/// Primeira procura depois do boot (não disputa CPU com o carregamento do modelo).
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Estado do Tauri: o item do tray e a atualização encontrada (se houver).
pub struct Updates {
    item: MenuItem<Wry>,
    pending: Mutex<Option<Update>>,
    /// O que o item mostra agora, para refazer o texto quando o idioma muda.
    label: Mutex<UpdateLabel>,
}

impl Updates {
    pub fn new(item: MenuItem<Wry>) -> Self {
        Self {
            item,
            pending: Mutex::new(None),
            label: Mutex::new(UpdateLabel::Check),
        }
    }

    /// Mostra `label` no idioma `lang` e lembra dele.
    fn show(&self, lang: Lang, label: UpdateLabel) {
        let _ = self.item.set_text(tray_i18n::update(lang, &label));
        *self.label.lock().unwrap() = label;
    }

    /// Refaz o texto atual no novo idioma.
    pub fn relabel(&self, lang: Lang) {
        let label = self.label.lock().unwrap().clone();
        let _ = self.item.set_text(tray_i18n::update(lang, &label));
    }
}

/// Procura periodicamente em segundo plano.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio_sleep(FIRST_CHECK_DELAY).await;
        loop {
            check(&app).await;
            tokio_sleep(CHECK_EVERY).await;
        }
    });
}

async fn tokio_sleep(d: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(d)).await;
}

/// Procura agora; se achar, troca o texto do item do tray e avisa o hub.
pub async fn check(app: &AppHandle) {
    let found = match app.updater() {
        Ok(updater) => updater.check().await,
        Err(err) => {
            tracing::warn!(%err, "updater indisponível");
            return;
        }
    };
    let Some(state) = app.try_state::<Updates>() else {
        return;
    };
    match found {
        Ok(Some(update)) => {
            tracing::info!(version = %update.version, "atualização disponível");
            state.show(
                tray_i18n::current(app),
                UpdateLabel::Install(update.version.clone()),
            );
            let _ = app.emit(
                EVENT_AVAILABLE,
                serde_json::json!({ "version": update.version }),
            );
            *state.pending.lock().unwrap() = Some(update);
        }
        Ok(None) => {
            tracing::info!("nenhuma atualização");
            state.show(tray_i18n::current(app), UpdateLabel::UpToDate);
        }
        Err(err) => tracing::warn!(%err, "falha ao procurar atualização"),
    }
}

/// Clique no item do tray: instala a pendente (e reinicia) ou procura de novo.
pub fn on_click(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let pending = app
            .try_state::<Updates>()
            .and_then(|s| s.pending.lock().unwrap().take());
        let Some(update) = pending else {
            check(&app).await;
            return;
        };
        if let Some(state) = app.try_state::<Updates>() {
            state.show(tray_i18n::current(&app), UpdateLabel::Downloading);
        }
        tracing::info!(version = %update.version, "instalando atualização");
        match update.download_and_install(|_, _| {}, || {}).await {
            Ok(()) => app.restart(),
            Err(err) => {
                tracing::error!(%err, "atualização falhou");
                if let Some(state) = app.try_state::<Updates>() {
                    state.show(tray_i18n::current(&app), UpdateLabel::Failed);
                }
            }
        }
    });
}
