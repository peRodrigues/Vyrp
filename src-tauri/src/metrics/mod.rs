//! Métricas e diagnóstico (V22): agregação das latências do histórico e o texto de
//! "copiar diagnóstico".
//!
//! Tudo aqui é puro e **nunca vê texto do usuário**: a entrada são só números e códigos
//! (`HistoryMetric`), e o diagnóstico é montado de campos explícitos (versão, SO, motor,
//! modelo, política...). Quem ler o histórico para cá deve usar `HistoryRepo::metrics`,
//! que nem seleciona as colunas de texto.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Status que contam como ditado bem-sucedido (o resto é erro, contado pelo código).
/// `no_speech` (transcrição vazia/VAD) é "ok sem texto": hoje o orquestrador nem grava essa
/// linha, mas, se gravar, não conta como erro.
pub const OK_STATUSES: [&str; 3] = ["inserted", "undo", "no_speech"];

/// Uma linha do histórico reduzida ao que importa para métricas (sem texto, app ou URL).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryMetric {
    pub created_at: i64,
    pub duration_ms: i64,
    pub word_count: i64,
    pub latency_stt_ms: Option<i64>,
    pub latency_llm_ms: Option<i64>,
    /// Soltar a tecla → texto inserido.
    pub latency_total_ms: Option<i64>,
    /// 0 = sem reserva; >0 = o LLM falhou e valeu o texto sem limpeza.
    pub fallback_level: i64,
    pub status: String,
    pub engine: Option<String>,
    pub llm_provider: Option<String>,
}

impl HistoryMetric {
    pub fn is_ok(&self) -> bool {
        OK_STATUSES.contains(&self.status.as_str())
    }
}

/// Resumo de uma série de milissegundos.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// Quantas amostras (ditados que têm esse marco).
    pub count: u32,
    pub p50: Option<i64>,
    pub p95: Option<i64>,
    pub max: Option<i64>,
}

/// Percentil pelo método do posto mais próximo (`p` em 0..=100) numa lista **ordenada**.
pub fn percentile(sorted: &[i64], p: u32) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let p = p.min(100) as usize;
    // Posto mais próximo: ceil(p/100 * n), 1-based.
    let rank = (p * sorted.len()).div_ceil(100).max(1);
    Some(sorted[rank - 1])
}

pub fn summarize(values: impl IntoIterator<Item = i64>) -> Summary {
    let mut v: Vec<i64> = values.into_iter().filter(|x| *x >= 0).collect();
    v.sort_unstable();
    Summary {
        count: v.len() as u32,
        p50: percentile(&v, 50),
        p95: percentile(&v, 95),
        max: v.last().copied(),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Aggregate {
    pub total: u32,
    pub ok: u32,
    pub failed: u32,
    /// Ditados em que o LLM foi usado (ou tentado).
    pub llm_used: u32,
    /// Ditados em que o LLM falhou e o texto saiu sem limpeza.
    pub llm_fallbacks: u32,
    pub stt: Summary,
    pub llm: Summary,
    pub total_latency: Summary,
    /// Erros por código (status), do mais frequente para o menos.
    pub errors: Vec<ErrorCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorCount {
    pub code: String,
    pub count: u32,
}

/// Agrega as linhas (das mais recentes ou não: a ordem não importa).
pub fn aggregate(rows: &[HistoryMetric]) -> Aggregate {
    let mut errors: BTreeMap<&str, u32> = BTreeMap::new();
    let mut agg = Aggregate {
        total: rows.len() as u32,
        ..Aggregate::default()
    };
    for row in rows {
        if row.is_ok() {
            agg.ok += 1;
        } else {
            agg.failed += 1;
            *errors.entry(row.status.as_str()).or_default() += 1;
        }
        if row.llm_provider.is_some() {
            agg.llm_used += 1;
        }
        if row.fallback_level > 0 {
            agg.llm_fallbacks += 1;
        }
    }
    agg.stt = summarize(rows.iter().filter_map(|r| r.latency_stt_ms));
    agg.llm = summarize(rows.iter().filter_map(|r| r.latency_llm_ms));
    agg.total_latency = summarize(rows.iter().filter_map(|r| r.latency_total_ms));
    let mut list: Vec<ErrorCount> = errors
        .into_iter()
        .map(|(code, count)| ErrorCount {
            code: code.to_string(),
            count,
        })
        .collect();
    list.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.code.cmp(&b.code)));
    agg.errors = list;
    agg
}

/// Últimos erros (só código e quando), do mais novo para o mais velho.
pub fn recent_errors(rows: &[HistoryMetric], limit: usize) -> Vec<(i64, String)> {
    let mut failed: Vec<&HistoryMetric> = rows.iter().filter(|r| !r.is_ok()).collect();
    failed.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    failed
        .into_iter()
        .take(limit)
        .map(|r| (r.created_at, r.status.clone()))
        .collect()
}

// ---------------------------------------------------------------- diagnóstico

/// Tudo que entra no "copiar diagnóstico". São campos explícitos e já sem segredo: não há
/// como o texto do usuário ou uma chave chegar aqui por acidente.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diagnostics {
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub cpu: String,
    pub logical_cores: usize,
    /// "cpu", "cuda" ou "vulkan" (o backend do whisper deste binário).
    pub stt_backend: String,
    pub stt_model: String,
    pub stt_state: String,
    pub stt_policy: String,
    pub stt_cloud: String,
    pub llm_provider: String,
    pub llm_model: String,
    pub llm_custom_url: bool,
    pub cleanup: String,
    pub style: String,
    pub insert_method: String,
    pub retention: String,
    pub languages: Vec<String>,
    pub microphone: String,
    pub warm_mic: bool,
    pub sounds: bool,
    pub media_mode: String,
    pub settings_version: u32,
    pub shortcut_ptt: String,
    pub keys_configured: Vec<(String, bool)>,
    pub aggregate: Aggregate,
    pub sample_size: usize,
    /// (quando em ms Unix, código).
    pub recent_errors: Vec<(i64, String)>,
}

fn ms(v: Option<i64>) -> String {
    v.map_or("—".to_string(), |x| format!("{x} ms"))
}

fn summary_line(label: &str, s: &Summary) -> String {
    format!(
        "  {label}: p50 {} · p95 {} · max {} ({} samples)",
        ms(s.p50),
        ms(s.p95),
        ms(s.max),
        s.count
    )
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

impl Diagnostics {
    /// Texto para colar num chamado de suporte. Fixo em inglês (não segue o idioma da tela), sem
    /// texto de ditado e sem chaves.
    pub fn render(&self) -> String {
        let mut out = vec![
            "== Vyrp diagnostics ==".to_string(),
            format!("Version: {}", self.app_version),
            format!("System: {} ({})", self.os, self.arch),
            format!("CPU: {} ({} logical cores)", self.cpu, self.logical_cores),
            String::new(),
            "-- Transcription --".to_string(),
            format!(
                "Local engine: whisper ({}), model {} [{}]",
                self.stt_backend, self.stt_model, self.stt_state
            ),
            format!("Policy: {} (cloud: {})", self.stt_policy, self.stt_cloud),
            format!("Languages: {}", self.languages.join(", ")),
            String::new(),
            "-- AI cleanup --".to_string(),
            format!(
                "Provider: {} · model: {} · custom URL: {}",
                self.llm_provider,
                self.llm_model,
                yes_no(self.llm_custom_url)
            ),
            format!("Level: {} · style: {}", self.cleanup, self.style),
            String::new(),
            "-- Relevant settings --".to_string(),
            format!("Settings v{}", self.settings_version),
            format!("Microphone: {}", self.microphone),
            format!(
                "Warm mic: {} · sounds: {} · media: {}",
                yes_no(self.warm_mic),
                yes_no(self.sounds),
                self.media_mode
            ),
            format!(
                "Shortcut: {} · insert: {} · history: {}",
                self.shortcut_ptt, self.insert_method, self.retention
            ),
        ];
        let keys: Vec<String> = self
            .keys_configured
            .iter()
            .map(|(name, set)| format!("{name}: {}", yes_no(*set)))
            .collect();
        out.push(format!("Keys configured: {}", keys.join(", ")));
        out.push(String::new());
        out.push(format!(
            "-- Latencies (last {} dictations) --",
            self.sample_size
        ));
        let a = &self.aggregate;
        out.push(format!(
            "Dictations: {} · ok: {} · failed: {} · used AI: {} · AI fell back: {}",
            a.total, a.ok, a.failed, a.llm_used, a.llm_fallbacks
        ));
        out.push(summary_line("Transcription", &a.stt));
        out.push(summary_line("AI cleanup", &a.llm));
        out.push(summary_line("Total (release → pasted)", &a.total_latency));
        out.push(String::new());
        out.push("-- Errors --".to_string());
        if a.errors.is_empty() {
            out.push("  none".to_string());
        }
        for e in &a.errors {
            out.push(format!("  {}: {}", e.code, e.count));
        }
        if !self.recent_errors.is_empty() {
            out.push("Recent errors (code, epoch ms):".to_string());
            for (when, code) in &self.recent_errors {
                out.push(format!("  {code} @ {when}"));
            }
        }
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, stt: Option<i64>, llm: Option<i64>, total: Option<i64>) -> HistoryMetric {
        HistoryMetric {
            created_at: 1_000,
            duration_ms: 2_000,
            word_count: 5,
            latency_stt_ms: stt,
            latency_llm_ms: llm,
            latency_total_ms: total,
            fallback_level: 0,
            status: status.into(),
            engine: Some("whisper".into()),
            llm_provider: None,
        }
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let v: Vec<i64> = (1..=100).collect();
        assert_eq!(percentile(&v, 50), Some(50));
        assert_eq!(percentile(&v, 95), Some(95));
        assert_eq!(percentile(&v, 100), Some(100));
        assert_eq!(percentile(&v, 0), Some(1));
        assert_eq!(percentile(&[7], 95), Some(7));
        assert_eq!(percentile(&[], 50), None);
        // 10 amostras: p95 = 10ª, p50 = 5ª.
        let ten: Vec<i64> = (1..=10).map(|x| x * 10).collect();
        assert_eq!(
            (percentile(&ten, 50), percentile(&ten, 95)),
            (Some(50), Some(100))
        );
    }

    #[test]
    fn summarize_sorts_and_ignores_negative_values() {
        let s = summarize([300, 100, 200, -5, 400]);
        assert_eq!(s.count, 4);
        assert_eq!((s.p50, s.p95, s.max), (Some(200), Some(400), Some(400)));
        assert_eq!(summarize([]), Summary::default());
    }

    #[test]
    fn aggregate_counts_phases_errors_and_fallbacks() {
        let mut with_llm = row("inserted", Some(500), Some(800), Some(1_400));
        with_llm.llm_provider = Some("Anthropic".into());
        let mut fell_back = row("inserted", Some(600), None, Some(700));
        fell_back.llm_provider = Some("Anthropic".into());
        fell_back.fallback_level = 1;
        let rows = vec![
            row("inserted", Some(400), None, Some(450)),
            with_llm,
            fell_back,
            row("stt_failed", None, None, None),
            row("stt_failed", None, None, None),
            row("insert_failed", Some(450), None, Some(500)),
            row("undo", None, None, None),
        ];
        let a = aggregate(&rows);
        assert_eq!((a.total, a.ok, a.failed), (7, 4, 3));
        assert!(row("no_speech", None, None, None).is_ok());
        assert!(!row("blocked", None, None, None).is_ok());
        assert_eq!((a.llm_used, a.llm_fallbacks), (2, 1));
        assert_eq!(a.stt.count, 4);
        assert_eq!(a.llm.count, 1);
        assert_eq!(a.llm.p50, Some(800));
        assert_eq!(a.total_latency.count, 4);
        assert_eq!(
            a.errors,
            vec![
                ErrorCount {
                    code: "stt_failed".into(),
                    count: 2
                },
                ErrorCount {
                    code: "insert_failed".into(),
                    count: 1
                },
            ]
        );
    }

    #[test]
    fn empty_history_gives_an_empty_aggregate() {
        assert_eq!(aggregate(&[]), Aggregate::default());
    }

    #[test]
    fn recent_errors_are_newest_first_and_codes_only() {
        let mut a = row("stt_failed", None, None, None);
        a.created_at = 10;
        let mut b = row("insert_failed", None, None, None);
        b.created_at = 30;
        let mut ok = row("inserted", None, None, None);
        ok.created_at = 99;
        let mut c = row("stt_failed", None, None, None);
        c.created_at = 20;
        assert_eq!(
            recent_errors(&[a, b, ok, c], 2),
            vec![
                (30, "insert_failed".to_string()),
                (20, "stt_failed".to_string())
            ]
        );
    }

    fn sample() -> Diagnostics {
        Diagnostics {
            app_version: "0.1.0".into(),
            os: "Windows 11".into(),
            arch: "x86_64".into(),
            cpu: "Intel(R) Core(TM) i7".into(),
            logical_cores: 16,
            stt_backend: "cpu".into(),
            stt_model: "small-q5_1".into(),
            stt_state: "ready".into(),
            stt_policy: "local_only".into(),
            stt_cloud: "groq".into(),
            llm_provider: "Anthropic".into(),
            llm_model: "claude-haiku-5-5".into(),
            llm_custom_url: false,
            cleanup: "light".into(),
            style: "formal".into(),
            insert_method: "auto".into(),
            retention: "keep".into(),
            languages: vec!["pt".into()],
            microphone: "Mic (USB)".into(),
            warm_mic: false,
            sounds: true,
            media_mode: "off".into(),
            settings_version: 5,
            shortcut_ptt: "Ctrl + Win".into(),
            keys_configured: vec![("Groq".into(), false), ("Anthropic".into(), true)],
            aggregate: aggregate(&[row("inserted", Some(400), Some(700), Some(1_200))]),
            sample_size: 50,
            recent_errors: vec![(1_700_000_000_000, "stt_failed".into())],
        }
    }

    #[test]
    fn diagnostics_text_has_every_section_and_the_numbers() {
        let text = sample().render();
        for needle in [
            "Version: 0.1.0",
            "whisper (cpu), model small-q5_1 [ready]",
            "Provider: Anthropic · model: claude-haiku-5-5 · custom URL: no",
            "Keys configured: Groq: no, Anthropic: yes",
            "Total (release → pasted): p50 1200 ms",
            "stt_failed @ 1700000000000",
        ] {
            assert!(text.contains(needle), "faltou: {needle}\n{text}");
        }
    }

    #[test]
    fn diagnostics_only_say_whether_keys_exist() {
        // Os campos do diagnóstico não têm onde guardar um valor de chave: só sim/não.
        let text = sample().render();
        assert!(!text.contains("sk-") && !text.contains("gsk_"));
        assert!(text.contains("Groq: no"));
    }

    #[test]
    fn empty_numbers_render_as_dashes() {
        let mut d = sample();
        d.aggregate = aggregate(&[]);
        d.recent_errors.clear();
        let text = d.render();
        assert!(text.contains("p50 — · p95 — · max — (0 samples)"));
        assert!(text.contains("  none"));
    }
}
