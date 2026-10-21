//! Commands só do hub (V17a/V20): microfones, padrões, histórico e testes de áudio do onboarding.
//!
//! Settings e chaves de API têm os próprios commands em `settings`; o dicionário, em
//! `storage::dictionary_commands`. Aqui ficam as leituras que a tela precisa e que não
//! pertencem a nenhum desses módulos.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, State};

use crate::audio::AudioService;
use crate::core::types::{
    AudioClip, CleanRequest, CleanupLevel, LlmProvider, SAMPLE_RATE, SttEngine, SttOptions,
    Transcript, WritingStyle,
};
use crate::core::ui_error::UiError;
use crate::llm::{LlmCleaner, LlmConfig, LlmKind};
use crate::metrics::{Aggregate, Diagnostics, HistoryMetric, aggregate, recent_errors};
use crate::settings::secrets::{Provider, Secrets};
use crate::settings::{Settings, SettingsStore};
use crate::storage::{Db, HistoryEntry};
use crate::stt::LocalStt;
use crate::stt::cloud::{AppKeys, CloudConfig, CloudProvider, CloudStt, KeyProvider};

// ---------------------------------------------------------------- histórico

/// Linha do histórico para a UI: sem o blob de áudio, com o texto "final" já escolhido.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: String,
    pub created_at: i64,
    pub raw_text: String,
    /// O que o usuário recebeu: editado > colado > limpo > cru.
    pub final_text: String,
    pub app: Option<String>,
    pub language: Option<String>,
    pub duration_ms: i64,
    pub word_count: i64,
    pub latency_total_ms: Option<i64>,
    pub status: String,
    pub reverted_ai: bool,
    pub has_audio: bool,
}

impl From<HistoryEntry> for HistoryItem {
    fn from(e: HistoryEntry) -> Self {
        let final_text = e
            .edited_text
            .or(e.inserted_text)
            .or(e.clean_text)
            .unwrap_or_else(|| e.raw_text.clone());
        Self {
            id: e.id,
            created_at: e.created_at,
            raw_text: e.raw_text,
            final_text,
            app: e.app,
            language: e.language,
            duration_ms: e.duration_ms,
            word_count: e.word_count,
            latency_total_ms: e.latency_total_ms,
            status: e.status,
            reverted_ai: e.reverted_ai,
            has_audio: e.audio.is_some(),
        }
    }
}

/// Teto por página, para a UI não pedir o banco inteiro de uma vez.
const MAX_PAGE: u32 = 200;

pub fn history_page(
    db: &Db,
    search: Option<&str>,
    limit: u32,
    offset: u32,
) -> crate::storage::Result<Vec<HistoryItem>> {
    Ok(db
        .history()
        .list(search, limit.clamp(1, MAX_PAGE), offset)?
        .into_iter()
        .map(HistoryItem::from)
        .collect())
}

#[tauri::command]
pub fn history_list(
    db: State<'_, Db>,
    search: Option<String>,
    limit: u32,
    offset: u32,
) -> Result<Vec<HistoryItem>, String> {
    history_page(&db, search.as_deref(), limit, offset).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn history_delete(db: State<'_, Db>, id: String) -> Result<bool, String> {
    db.history().delete(&id).map_err(|e| e.to_string())
}

/// Apaga os áudios guardados no histórico (V21); o texto fica.
#[tauri::command]
pub fn history_clear_audio(db: State<'_, Db>) -> Result<u64, String> {
    db.history().clear_audio().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn history_clear(db: State<'_, Db>) -> Result<u64, String> {
    db.history().clear().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- microfones e padrões

#[tauri::command]
pub fn list_microphones(audio: State<'_, AudioService>) -> Vec<String> {
    audio.devices()
}

/// Settings de fábrica, para os botões "restaurar padrão" da tela.
#[tauri::command]
pub fn settings_defaults() -> Settings {
    Settings::default()
}

// ---------------------------------------------------------------- onboarding: testes de áudio

/// Começa a ouvir só para mostrar o medidor de nível (evento `dictation://level`).
/// Pare com [`audio_monitor_stop`]; nada é guardado.
#[tauri::command]
pub fn audio_monitor_start(audio: State<'_, AudioService>) {
    // Não começa por cima de um ditado (V11b); o hub recebe `audio://monitor` se cair.
    audio.begin_monitor();
}

#[tauri::command]
pub fn audio_monitor_stop(audio: State<'_, AudioService>) {
    // Só para o medidor: nunca cancela um ditado em curso (V11b).
    audio.cancel_monitor();
}

/// Resultado do ditado de teste do onboarding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DictationTest {
    pub text: String,
    pub language: Option<String>,
    pub audio_ms: u64,
    pub decode_ms: u64,
}

impl DictationTest {
    pub fn new(transcript: Transcript, audio_ms: u64) -> Self {
        Self {
            text: transcript.text.trim().to_string(),
            language: transcript.language,
            audio_ms,
            decode_ms: transcript.timings.decode_ms,
        }
    }
}

/// Começa o ditado de teste pelo orquestrador (V11b): grava até o
/// [`dictation_test_stop`]. O overlay acompanha pelos eventos de estado e de nível.
#[tauri::command]
pub fn dictation_test_start(
    orchestrator: State<'_, Arc<crate::dictation::Orchestrator>>,
) -> Result<(), UiError> {
    orchestrator.test_start()
}

/// Para de gravar e transcreve pelo pipeline do ditado (VAD + STT + vocabulário), sem colar.
#[tauri::command]
pub async fn dictation_test_stop(app: AppHandle) -> Result<DictationTest, UiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let orchestrator = app.state::<Arc<crate::dictation::Orchestrator>>();
        orchestrator
            .test_stop()
            .map(|done| DictationTest::new(done.transcript, done.audio_ms))
    })
    .await
    .map_err(|e| UiError::raw(e.to_string()))?
}

// ---------------------------------------------------------------- V17b: testar provedores

/// Qual provedor testar. Serializa como `{"kind":"stt_cloud","provider":"groq"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "provider", rename_all = "snake_case")]
pub enum ProviderTarget {
    SttCloud(CloudProvider),
    Llm(LlmKind),
}

/// Resultado do teste: sempre devolvido (falha de rede/chave não é erro do command), sem
/// nenhum dado secreto. `message` é código + parâmetros: a tela traduz (`uierr.<code>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderTestResult {
    pub ok: bool,
    pub message: UiError,
    pub ms: u64,
}

/// Prazo do teste: mais folgado que o do ditado (1ª conexão, DNS, TLS).
const TEST_TIMEOUT: Duration = Duration::from_secs(8);

/// 0,6 s de um tom suave a 16 kHz: basta para o provedor autenticar e responder.
pub fn tone_clip() -> AudioClip {
    let samples = (SAMPLE_RATE as f32 * 0.6) as usize;
    let pcm = (0..samples)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            ((t * 440.0 * std::f32::consts::TAU).sin() * 3_000.0) as i16
        })
        .collect();
    AudioClip::new(pcm)
}

/// Pede a limpeza de uma frase curta ao provedor.
pub async fn test_llm(provider: &dyn LlmProvider) -> ProviderTestResult {
    let req = CleanRequest {
        raw: "oi tudo bem queria testar a limpeza".into(),
        level: CleanupLevel::Light,
        style: WritingStyle::Formal,
        ..Default::default()
    };
    let started = Instant::now();
    let result = provider.clean(&req).await;
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(text) => ProviderTestResult {
            ok: true,
            message: UiError::new(
                "test.llm_ok",
                format!("Funcionou. Resposta: \u{201c}{}\u{201d}", text.trim()),
            )
            .with("text", text.trim()),
            ms,
        },
        Err(err) => ProviderTestResult {
            ok: false,
            message: err.ui(),
            ms,
        },
    }
}

/// Manda um áudio curto ao provedor de transcrição. Texto vazio também conta como sucesso:
/// o objetivo é validar chave, rede e prazo, não reconhecer fala.
pub async fn test_stt(engine: &dyn SttEngine) -> ProviderTestResult {
    let started = Instant::now();
    let result = engine
        .transcribe(&tone_clip(), &SttOptions::default())
        .await;
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(_) => ProviderTestResult {
            ok: true,
            message: UiError::new("test.stt_ok", "Funcionou: o provedor respondeu."),
            ms,
        },
        Err(err) => ProviderTestResult {
            ok: false,
            message: err.ui(),
            ms,
        },
    }
}

/// Testa o provedor com a chave guardada no Credential Manager e o modelo do Settings.
#[tauri::command]
pub async fn provider_test(app: AppHandle, target: ProviderTarget) -> ProviderTestResult {
    let settings = app.state::<SettingsStore>().get();
    let keys: Arc<dyn KeyProvider> = Arc::new(AppKeys(app.clone()));
    match target {
        ProviderTarget::SttCloud(provider) => {
            let engine = CloudStt::new(
                provider,
                keys,
                CloudConfig {
                    timeout: TEST_TIMEOUT,
                    ..CloudConfig::default()
                },
            );
            test_stt(&engine).await
        }
        ProviderTarget::Llm(kind) => {
            let model = (settings.llm_provider == Some(kind))
                .then(|| settings.llm_model.clone())
                .flatten()
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty());
            let url = (settings.llm_provider == Some(kind))
                .then(|| settings.llm_url.clone())
                .flatten()
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty())
                .map(|u| crate::factory::resolve_llm_url(kind, &u));
            let cleaner = LlmCleaner::new(
                kind,
                keys,
                LlmConfig {
                    timeout: TEST_TIMEOUT,
                    model,
                    url,
                },
            );
            test_llm(&cleaner).await
        }
    }
}

// ---------------------------------------------------------------- V22: métricas e diagnóstico

/// Quantos ditados o painel olha, no máximo.
const MAX_METRICS_SAMPLE: u32 = 500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentError {
    pub created_at: i64,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsReport {
    pub aggregate: Aggregate,
    pub recent_errors: Vec<RecentError>,
    /// Quantas linhas foram lidas (pode ser menos que o pedido).
    pub sample_size: usize,
}

pub fn build_metrics(rows: &[HistoryMetric]) -> MetricsReport {
    MetricsReport {
        aggregate: aggregate(rows),
        recent_errors: recent_errors(rows, 10)
            .into_iter()
            .map(|(created_at, code)| RecentError { created_at, code })
            .collect(),
        sample_size: rows.len(),
    }
}

/// Latência por fase, p50/p95 e erros por código dos últimos `limit` ditados.
#[tauri::command]
pub fn metrics_report(db: State<'_, Db>, limit: u32) -> Result<MetricsReport, String> {
    let rows = db
        .history()
        .metrics(limit.clamp(1, MAX_METRICS_SAMPLE))
        .map_err(|e| e.to_string())?;
    Ok(build_metrics(&rows))
}

/// Nome do processador, como o Windows informa ao processo.
fn cpu_name() -> String {
    std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "desconhecido".into())
}

/// "Microsoft Windows [versão 10.0.x]" (via `ver`, sem abrir janela); cai no nome genérico.
fn os_name() -> String {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if let Ok(out) = std::process::Command::new("cmd")
            .args(["/c", "ver"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !text.is_empty() {
                return text;
            }
        }
    }
    std::env::consts::OS.to_string()
}

fn snake<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "?".into())
}

/// Texto de "copiar diagnóstico": sem texto de ditado, sem chaves, sem endereços.
#[tauri::command]
pub fn diagnostics_text(app: AppHandle, limit: u32) -> Result<String, String> {
    let settings = app.state::<SettingsStore>().get();
    let rows = app
        .try_state::<Db>()
        .map(|db| db.history().metrics(limit.clamp(1, MAX_METRICS_SAMPLE)))
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let report = build_metrics(&rows);
    let stt = app.try_state::<LocalStt>().map(|s| s.0.status());
    let keys = app.try_state::<Secrets>();
    let has_key = |p: Provider| keys.as_ref().and_then(|k| k.has(p).ok()).unwrap_or(false);
    let diagnostics = Diagnostics {
        app_version: app.package_info().version.to_string(),
        os: os_name(),
        arch: std::env::consts::ARCH.into(),
        cpu: cpu_name(),
        logical_cores: std::thread::available_parallelism().map_or(0, |n| n.get()),
        stt_backend: crate::stt::whisper::backend().into(),
        stt_model: settings.stt_model.clone(),
        stt_state: stt.map_or("unavailable".into(), |s| snake(&s.state)),
        stt_policy: snake(&settings.stt_policy),
        stt_cloud: snake(&settings.stt_cloud),
        llm_provider: settings
            .llm_provider
            .map_or("off".into(), |k| k.name().to_string()),
        llm_model: settings
            .llm_model
            .clone()
            .unwrap_or_else(|| "provider default".into()),
        llm_custom_url: settings.llm_url.is_some(),
        cleanup: snake(&settings.cleanup),
        style: snake(&settings.style),
        insert_method: snake(&settings.insert_method),
        retention: snake(&settings.history_retention),
        languages: settings.dictation_languages.clone(),
        microphone: settings
            .microphone
            .clone()
            .unwrap_or_else(|| "system default".into()),
        warm_mic: settings.warm_mic,
        sounds: settings.sounds,
        media_mode: snake(&settings.media_mode),
        settings_version: settings.version,
        shortcut_ptt: settings.gestures.ptt.label(),
        keys_configured: vec![
            ("Groq".into(), has_key(Provider::Groq)),
            ("OpenAI".into(), has_key(Provider::OpenAi)),
            ("Anthropic".into(), has_key(Provider::Anthropic)),
        ],
        aggregate: report.aggregate,
        sample_size: report.sample_size,
        recent_errors: report
            .recent_errors
            .into_iter()
            .map(|e| (e.created_at, e.code))
            .collect(),
    };
    Ok(diagnostics.render())
}

/// O log mais novo da pasta (`vyrp.log.AAAA-MM-DD`, rotação diária): o nome ordena por data.
pub fn newest_log_file(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("vyrp.log"))
        })
        .max()
}

/// Nome do arquivo exportado: `vyrp-log-<época em ms>.log`.
pub fn export_file_name(now_ms: u128) -> String {
    format!("vyrp-log-{now_ms}.log")
}

/// Copia o log mais novo para a pasta Downloads e devolve o caminho. (Os logs podem ter
/// nomes de modelo e erros, mas nunca o texto dos ditados do dia a dia; os modos `--diag-*`
/// podem registrar a transcrição: confira antes de compartilhar.)
#[tauri::command]
pub fn export_log(app: AppHandle) -> Result<String, UiError> {
    let dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    let source = newest_log_file(&dir)
        .ok_or_else(|| UiError::new("log.none", "Ainda não há arquivo de log."))?;
    let target_dir = app
        .path()
        .download_dir()
        .map_err(|_| UiError::new("log.no_downloads", "Não encontrei a pasta Downloads."))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let target = target_dir.join(export_file_name(now));
    std::fs::copy(&source, &target).map_err(|e| e.to_string())?;
    Ok(target.display().to_string())
}

/// Abre a pasta de logs no Explorer.
#[tauri::command]
pub fn open_log_dir(app: AppHandle) -> Result<(), UiError> {
    let dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let opener = "explorer";
    #[cfg(not(windows))]
    let opener = "xdg-open";
    std::process::Command::new(opener)
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| UiError::raw(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::VyrpError;

    #[test]
    fn final_text_prefers_edited_then_inserted_then_clean_then_raw() {
        let mut e = HistoryEntry::new("cru", 1);
        assert_eq!(HistoryItem::from(e.clone()).final_text, "cru");
        e.clean_text = Some("limpo".into());
        assert_eq!(HistoryItem::from(e.clone()).final_text, "limpo");
        e.inserted_text = Some("colado".into());
        assert_eq!(HistoryItem::from(e.clone()).final_text, "colado");
        e.edited_text = Some("editado".into());
        let item = HistoryItem::from(e);
        assert_eq!(item.final_text, "editado");
        assert_eq!(item.raw_text, "cru");
    }

    #[test]
    fn audio_blob_never_reaches_the_ui() {
        let mut e = HistoryEntry::new("oi", 1);
        e.audio = Some(vec![1; 1024]);
        let item = HistoryItem::from(e);
        assert!(item.has_audio);
        assert!(!serde_json::to_string(&item).unwrap().contains("audio\":["));
    }

    #[test]
    fn page_is_newest_first_filtered_and_capped() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..3 {
            db.history()
                .insert(&HistoryEntry::new(format!("ditado {i}"), i))
                .unwrap();
        }
        let all = history_page(&db, None, 50, 0).unwrap();
        assert_eq!(all[0].raw_text, "ditado 2");
        assert_eq!(history_page(&db, Some("ditado 1"), 50, 0).unwrap().len(), 1);
        // limit 0 vira 1; limite enorme é cortado em MAX_PAGE.
        assert_eq!(history_page(&db, None, 0, 0).unwrap().len(), 1);
        assert_eq!(history_page(&db, None, 9_999, 0).unwrap().len(), 3);
    }

    #[test]
    fn clearing_audio_keeps_the_text() {
        let db = Db::open_in_memory().unwrap();
        let mut with_audio = HistoryEntry::new("texto fica", 1);
        with_audio.audio = Some(vec![1, 2, 3]);
        let without = HistoryEntry::new("sem audio", 2);
        db.history().insert(&with_audio).unwrap();
        db.history().insert(&without).unwrap();
        assert_eq!(db.history().clear_audio().unwrap(), 1);
        let rows = db.history().list(None, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.audio.is_none()));
        assert!(rows.iter().any(|r| r.raw_text == "texto fica"));
        assert_eq!(db.history().clear_audio().unwrap(), 0);
    }

    #[test]
    fn dictation_test_trims_the_text_and_keeps_the_timings() {
        let transcript = Transcript {
            text: "  olá mundo \n".into(),
            language: Some("pt".into()),
            segments: vec![],
            timings: crate::core::types::SttTimings {
                load_ms: 0,
                decode_ms: 321,
            },
        };
        let test = DictationTest::new(transcript, 1_800);
        assert_eq!(test.text, "olá mundo");
        assert_eq!(test.language.as_deref(), Some("pt"));
        assert_eq!((test.audio_ms, test.decode_ms), (1_800, 321));
    }

    // ---- V17b

    struct FakeLlm(Result<&'static str, &'static str>);

    #[async_trait::async_trait]
    impl LlmProvider for FakeLlm {
        async fn clean(&self, req: &CleanRequest) -> crate::core::types::Result<String> {
            assert!(!req.raw.is_empty());
            self.0
                .map(str::to_string)
                .map_err(|e| VyrpError::Llm(e.into()))
        }
    }

    struct FakeStt(Result<&'static str, &'static str>);

    #[async_trait::async_trait]
    impl SttEngine for FakeStt {
        async fn transcribe(
            &self,
            clip: &AudioClip,
            _: &SttOptions,
        ) -> crate::core::types::Result<Transcript> {
            assert!(clip.duration_ms() >= 500);
            self.0
                .map(|t| Transcript {
                    text: t.into(),
                    ..Default::default()
                })
                .map_err(|e| VyrpError::Stt(e.into()))
        }
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tauri::async_runtime::block_on(f)
    }

    #[test]
    fn llm_test_reports_the_answer_or_the_typed_error() {
        let ok = block_on(test_llm(&FakeLlm(Ok(" Olá, tudo bem. "))));
        assert!(ok.ok);
        assert!(ok.message.to_string().contains("Olá, tudo bem."));
        let err = block_on(test_llm(&FakeLlm(Err("chave de API recusada pelo OpenAI"))));
        assert!(!err.ok);
        assert!(err.message.to_string().contains("chave de API recusada"));
    }

    #[test]
    fn stt_test_succeeds_even_with_empty_text_and_fails_on_error() {
        assert!(block_on(test_stt(&FakeStt(Ok("")))).ok);
        let err = block_on(test_stt(&FakeStt(Err("tempo esgotado falando com o Groq"))));
        assert!(!err.ok && err.message.to_string().contains("tempo esgotado"));
    }

    #[test]
    fn tone_clip_is_long_enough_and_audible() {
        let clip = tone_clip();
        assert!(clip.duration_ms() >= 500);
        assert!(clip.pcm.iter().any(|s| s.unsigned_abs() > 1_000));
    }

    #[test]
    fn provider_target_wire_format_matches_the_ui() {
        let stt = serde_json::to_value(ProviderTarget::SttCloud(CloudProvider::OpenAi)).unwrap();
        assert_eq!(
            stt,
            serde_json::json!({"kind": "stt_cloud", "provider": "open_ai"})
        );
        let llm: ProviderTarget =
            serde_json::from_value(serde_json::json!({"kind": "llm", "provider": "ollama"}))
                .unwrap();
        assert_eq!(llm, ProviderTarget::Llm(LlmKind::Ollama));
    }

    // ---- V22

    fn metric(status: &str, total: Option<i64>) -> HistoryMetric {
        HistoryMetric {
            created_at: 5,
            duration_ms: 1_000,
            word_count: 3,
            latency_stt_ms: Some(300),
            latency_llm_ms: None,
            latency_total_ms: total,
            fallback_level: 0,
            status: status.into(),
            engine: None,
            llm_provider: None,
        }
    }

    #[test]
    fn metrics_report_builds_from_rows() {
        let rows = vec![
            metric("inserted", Some(800)),
            metric("stt_failed", None),
            metric("inserted", Some(1_200)),
        ];
        let report = build_metrics(&rows);
        assert_eq!(report.sample_size, 3);
        assert_eq!(report.aggregate.failed, 1);
        assert_eq!(report.aggregate.total_latency.p50, Some(800));
        assert_eq!(
            report.recent_errors,
            vec![RecentError {
                created_at: 5,
                code: "stt_failed".into()
            }]
        );
    }

    #[test]
    fn metrics_from_the_real_repo_have_no_text() {
        let db = Db::open_in_memory().unwrap();
        let mut e = HistoryEntry::new("frase privada", 7);
        e.latency_total_ms = Some(640);
        e.status = "inserted".into();
        db.history().insert(&e).unwrap();
        let rows = db.history().metrics(50).unwrap();
        let json = serde_json::to_string(&build_metrics(&rows)).unwrap();
        assert!(json.contains("640"));
        assert!(!json.contains("privada"));
    }

    #[test]
    fn newest_log_is_the_latest_dated_file() {
        let dir = std::env::temp_dir().join(format!("vyrp-logs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "vyrp.log.2026-10-19",
            "vyrp.log.2026-10-21",
            "vyrp.log.2026-10-20",
            "diag-1.wav",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let newest = newest_log_file(&dir).unwrap();
        assert_eq!(newest.file_name().unwrap(), "vyrp.log.2026-10-21");
        assert_eq!(newest_log_file(&dir.join("nao-existe")), None);
        assert_eq!(export_file_name(42), "vyrp-log-42.log");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snake_names_match_the_settings_json() {
        assert_eq!(
            snake(&crate::stt::cloud::SttPolicy::CloudWithFallback),
            "cloud_with_fallback"
        );
        assert_eq!(snake(&CleanupLevel::Medium), "medium");
    }

    #[test]
    fn defaults_command_matches_settings_default() {
        assert_eq!(settings_defaults(), Settings::default());
    }
}
