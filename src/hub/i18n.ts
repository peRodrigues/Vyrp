// i18n mínimo do hub: hoje só pt-BR, mas todo texto das telas novas passa por t(),
// então um segundo idioma é só outro dicionário (spec V17a).

const ptBR = {
  "tab.home": "Início",
  "tab.general": "Geral",
  "tab.history": "Histórico",
  "tab.dictionary": "Dicionário",
  "tab.advanced": "Avançado",

  "common.loading": "Carregando…",
  "common.yes": "Sim",
  "common.no": "Não",
  "common.remove": "Remover",
  "common.save": "Salvar",
  "common.cancel": "Cancelar",
  "common.delete": "Apagar",
  "common.copy": "Copiar",
  "common.copied": "Copiado!",
  "common.restoreDefault": "Restaurar padrão",
  "common.saveError": "Não foi possível salvar: {error}",

  "general.mic.title": "Microfone",
  "general.mic.label": "Microfone usado no ditado",
  "general.mic.default": "Padrão do sistema",
  "general.mic.absent": "{name} (desconectado)",
  "general.mic.warm": "Manter o microfone aberto entre ditados (pre-roll)",
  "general.mic.warmHint":
    "Evita cortar o começo da fala, mas o ícone de microfone do Windows fica aceso o tempo todo.",
  "general.mic.ranking": "Ordem de preferência",
  "general.mic.rankingHint":
    "Se o microfone escolhido não estiver conectado, o Vyrp usa o primeiro disponível desta lista.",
  "general.mic.offline": "desconectado",
  "general.mic.up": "Subir {name}",
  "general.mic.down": "Descer {name}",
  "general.mic.noDevices": "Nenhum microfone encontrado.",
  "general.mic.preroll": "Pre-roll (ms)",

  "general.lang.title": "Idiomas do ditado",
  "general.lang.hint": "Nenhum marcado: o Vyrp detecta o idioma sozinho.",

  "general.keys.title": "Atalhos",
  "general.keys.ptt": "Segurar para ditar",
  "general.keys.lock": "Travar o microfone (mãos livres)",
  "general.keys.lockNone": "Sem atalho de trava (só duplo toque)",
  "general.keys.cancel": "Cancelar",
  "general.keys.cancelFixed": "Esc",
  "general.keys.record": "Gravar atalho",
  "general.keys.recording": "Aperte as teclas… (Esc cancela)",
  "general.keys.minHold": "Segurar no mínimo (ms)",
  "general.keys.doubleTap": "Janela do duplo toque (ms)",
  "general.keys.debounce": "Pausa entre ditados (ms)",
  "general.keys.restore": "Restaurar atalhos padrão",

  "general.model.title": "Modelo de voz local",
  "general.model.soon":
    "O download de modelos chega junto com o reconhecimento de voz local. Por enquanto esta lista é só o catálogo.",
  "general.model.download": "Baixar",
  "general.model.installed": "Instalado",
  "general.model.size": "{mb} MB",

  "overlay.rest": "Vyrp",
  "overlay.arming": "Preparando…",
  "overlay.idle": "Pronto",
  "overlay.listening": "Ouvindo",
  "overlay.listeningLocked": "Ouvindo (mãos livres)",
  "overlay.locked": "Mãos livres",
  "overlay.processing": "Processando…",
  "overlay.inserting": "Colando…",
  "overlay.done": "Pronto",
  "overlay.error.mic_unavailable": "Microfone indisponível",
  "overlay.error.no_audio": "Nenhum áudio captado",
  "overlay.error.stt_failed": "Falha na transcrição",
  "overlay.error.llm_failed": "Falha na limpeza",
  "overlay.error.insert_failed": "Copiado. Cole manualmente.",
  "overlay.error.timeout": "Demorou demais",
  "overlay.hide1h": "1 h",
  "overlay.hide1hTitle": "Ocultar a barra por 1 hora",
  "overlay.hideForever": "Sempre",
  "overlay.hideForeverTitle": "Ocultar a barra (volta em Geral › Barra de ditado)",

  "general.overlay.title": "Barra de ditado",
  "general.overlay.show": "Mostrar a barra de ditado na tela",
  "general.overlay.hint": "Arraste a barra para mudar de lugar. Ocultada por 1 hora, ela volta sozinha.",

  "general.sounds": "Tocar sons ao começar e terminar o ditado",

  "advanced.retention.title": "Histórico de ditados",
  "advanced.retention.keep": "Guardar normalmente",
  "advanced.retention.day": "Apagar depois de 24 horas",
  "advanced.retention.never": "Nunca guardar",
  "advanced.retention.hint": "Ao escolher uma opção mais curta, o que passou do prazo é apagado na próxima abertura.",

  "advanced.voice.title": "Comandos de voz",
  "advanced.voice.hint": "Frases separadas por vírgula; funcionam em qualquer ponto da fala (apagar só vale sozinho).",
  "advanced.voice.insertNewline": "Quebra de linha",
  "advanced.voice.insertParagraph": "Novo parágrafo",
  "advanced.voice.insertText": "Insere “{text}”",
  "advanced.voice.undo": "Desfaz o último texto colado",
  "advanced.voice.off": "Desligar todos",
  "advanced.voice.empty": "Comandos de voz desligados.",

  "advanced.keys.title": "Chaves de API",
  "advanced.keys.hint":
    "Ficam no Gerenciador de Credenciais do Windows. O Vyrp nunca mostra a chave depois de salva.",
  "advanced.keys.set": "Configurada",
  "advanced.keys.unset": "Não configurada",
  "advanced.keys.placeholder": "Cole a chave aqui",

  "history.search": "Buscar no histórico",
  "history.clear": "Apagar tudo",
  "history.clearConfirm": "Apagar todo o histórico?",
  "history.empty": "Nenhum ditado guardado ainda.",
  "history.emptySearch": "Nenhum ditado encontrado para essa busca.",
  "history.off": "O histórico está desligado (Avançado › Histórico de ditados).",
  "history.showRaw": "Ver texto cru",
  "history.showFinal": "Ver texto final",
  "history.raw": "Cru",
  "history.final": "Final",
  "history.repaste": "Recolar",
  "history.repasteSoon": "Chega junto com a inserção de texto.",
  "history.more": "Carregar mais",
  "history.words": "{n} palavras",
  "history.words1": "1 palavra",
  "history.unknownApp": "app desconhecido",
} as const;

export type StringKey = keyof typeof ptBR;

const dictionaries = { "pt-BR": ptBR } as const;
let current: keyof typeof dictionaries = "pt-BR";

export function setLanguage(lang: keyof typeof dictionaries): void {
  current = lang;
}

/** Texto da tela; `{nome}` é trocado pelo valor de `vars.nome`. */
export function t(key: StringKey, vars?: Record<string, string | number>): string {
  const text: string = dictionaries[current][key];
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in vars ? String(vars[name]) : whole,
  );
}
