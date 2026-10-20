# Wispr Flow 1.6.1209 (Windows): especificação clean-room e proposta do Vyrp

> **Clean-room.** Este documento descreve **comportamento e arquitetura**
> observados no Wispr Flow instalado, em palavras próprias. Não contém
> código, prompts, assets nem strings longas do produto, e nenhum dado
> pessoal ou token da instalação analisada. O Vyrp é implementado a partir
> desta descrição, não do código do Wispr.
>
> **Fonte:** `%LOCALAPPDATA%\WisprFlow\app-1.6.1209`: `app.asar` extraído
> (bundle `main` + renderers), `Wispr Flow Helper.exe`, `.node`, pasta
> `migrations`. Esquema (sem linhas) de `%APPDATA%\Wispr Flow\flow.sqlite`,
> mais as chaves (sem valores) de `config.json` e `feature-flags-cache.json`.
> Análise estática; o app **não** foi executado para a análise.
>
> **Legenda:** **[O]** observado no artefato. **[I]** inferido. **[?]** desconhecido.

---

## 1. Mapa de features

### 1.1 Gatilho: push-to-talk e hands-free
- **PTT** [O]: segurar o atalho (padrão no Windows: `Ctrl+Win`), falar e soltar.
  O texto sai ao soltar.
- **Hands-free** ("popo", travado) [O]: duplo toque na tecla de PTT, ou atalho
  próprio (`Ctrl+Win+Space`). Termina com novo toque; `Esc` cancela.
- **Proteções** [O]:
  - debounce de 500 ms contra toque acidental;
  - aviso aos 19 min e parada forçada aos 20 min de gravação;
  - triplo toque reservado para outro modo (gravador de reunião).
- **Atalhos configuráveis** [O]:
  - combinações de até 3 teclas, com ao menos um modificador ou um botão
    extra do mouse (botões 3 a 10);
  - esquerda e direita são distinguíveis, e conflitos são validados;
  - persistidos como "códigos VK unidos por `+`" → ação.
- **Ações com atalho** [O]: ptt, popo, cancelar, colar último texto, copiar
  último texto, transform (e prompts salvos 1..9), ver diff, lens e notas.
- **Quem decide o gesto** [O]: o processo principal, não o nativo. O helper
  manda o fluxo cru de teclas, e o principal compara o conjunto pressionado
  com os chords. Há:
  - "upgrade" de chord (mesma ação, chord maior);
  - adiamento de decisão;
  - descarte de **chord fantasma**: depois de um atraso curto, pergunta ao
    nativo quais teclas estão *fisicamente* pressionadas.

### 1.2 Overlay ("flow bar")
- Barra pequena, por padrão na borda inferior; arrastável e ocultável por 1 h
  ou para sempre [O].
- **Estados** [O]: oculto, repouso, inicializando, pronto, ouvindo (PTT),
  ouvindo (travado), processando, polish em andamento/concluído/falhou,
  "limpeza concluída (ver diff)", lembrete de atalho, erro.
- **Sinais para a UI** [O]: nível de áudio (waveform), palavras por minuto,
  mic ativo, mic em silêncio e posição do dock.
- **Sons** [O]: início, fim, colar (só se o processamento demorou), trava
  hands-free e conquista. Opção de **pausar mídia** ou **silenciar todo o
  áudio** do sistema durante o ditado.

### 1.3 Captura de áudio
- **Onde captura** [O]: numa janela web oculta ("hub"), não em código
  nativo. O `AudioContext` e o worklet ficam **pré-construídos**; ao apertar
  a tecla só falta abrir o microfone.
- **Formato** [O]: mono, 16 kHz, PCM int16, blocos de 40 ms (640 amostras).
  Cancelamento de eco, supressão de ruído e ganho automático **desligados**:
  ruins para ASR.
- **Opus opcional** [O]: 64 kbps, pacotes de 20 ms. Cai para PCM se o
  encoder não produzir saída em 2 s.
- **Sem VAD e sem pre-roll no cliente** [O]:
  - blocos totalmente zerados antes do primeiro áudio real são descartados
    (aquecimento do driver);
  - fala anterior à abertura do mic se perde [I];
  - se o usuário solta e reaperta rápido, a abertura de mic "órfã" é
    reaproveitada por até 10 s.
- **Robustez** [O]:
  - retries de abertura em 250 ms e 750 ms;
  - aviso se a abertura levar mais de 8 s;
  - silêncio total por ~5 s gera aviso ("~6 s sem nenhum áudio" no começo),
    e ~15 s escala o aviso;
  - mic desconectado durante o ditado para a gravação e avisa;
  - um mic novo ligado gera uma pergunta ao usuário.
- **Escolha do microfone** [O]:
  - lista **ranqueada** pelo usuário, alimentada por um "quiz de microfone";
  - o mic embutido é recomendado; Bluetooth é detectado pelo rótulo;
  - modo notebook fechado ignora o mic embutido;
  - troca automática quando um mic de rank maior aparece;
  - headset Jabra: detecção de "fone na cabeça" (opt-in).

### 1.4 Speech-to-text (STT)
- **100% na nuvem** [O]: não há ASR local nem offline no desktop.
- **Transporte principal** [O]: stream **gRPC bidirecional** (HTTP/2) para um
  provedor de inferência hospedado, aberto **no momento em que a tecla é
  pressionada**, em paralelo com o mic.
- **Formato da requisição** [O]: uma única mensagem com campos opcionais.
  - **init**, enviado uma vez com:
    - identidade e versão;
    - idiomas;
    - dicionário pessoal e de time (favoritos primeiro);
    - snippets;
    - estilo e nível de limpeza;
    - nome do usuário (para assinaturas).
  - **context**, reenviável no meio da fala quando o foco muda.
  - **audio**: pacotes.
  - **commit**, na última mensagem.
- **Resposta** [O]: *heartbeat* (quantos segundos de áudio o servidor já
  recebeu), *state* (texto parcial cru e formatado) e *result* (texto final
  puro e HTML, idiomas detectados, confiança, tempos por etapa e origem do
  texto).
  - Os parciais **não são digitados**: servem de progresso e de
    classificação precoce de intenção.
- **Envio** [O]: fila de 1 slot que **concatena** pacotes enquanto um envio
  está em curso, sem timer. Keepalive HTTP/2 a cada 4 s.
- **Plano B** [O]: uma corrida por **POST HTTP** com o áudio inteiro (apenas
  áudio < 30 s), disparada quando:
  - não há heartbeat por 4 s durante a fala, ou por 2 s depois do commit;
  - o servidor está mais de 2 s atrasado em relação ao áudio enviado.

  Vence quem responder primeiro; respostas velhas são descartadas por id.
  Se tudo falhar, o ditado é refeito em blocos de 28 s com recuperação da
  cauda.
- **"ASR externo"** [O]: decidido no servidor; o cliente só registra que
  aconteceu.
- **Idiomas** [O]: ~105 códigos, com vários simultâneos. Lista vazia =
  autodetecção no servidor. Há um aviso quando o idioma falado difere do
  configurado.

### 1.5 Pós-processamento (limpeza por LLM)
- **Onde roda** [O]: o polimento (pontuação, maiúsculas, remoção de
  hesitações, autocorreção falada, listas, formato de e-mail) roda **no
  servidor, na mesma chamada da transcrição**. O cliente escolhe as etapas:
  transcrever, alinhar, formatar, minúsculas, menção Slack e assinatura.
- **Níveis de limpeza** [O]: nenhum, leve, médio e alto. A UI mostra um
  exemplo de antes/depois por nível. Nos níveis médio e alto a barra oferece
  um **diff** com aceitar/desfazer, mostrado poucas vezes para não irritar.
- **Formato por app** [O]: com IA, sem IA (cru), "mensagem casual" (só nomes
  próprios em maiúscula) ou tudo minúsculo.
- **Estilo por categoria de app** [O]: trabalho, e-mail, pessoal e outros,
  cada um com lista de apps. Estilos: formal, casual, muito casual e
  animado. O onboarding pergunta um estilo por categoria; um modo
  experimental detecta a formalidade da conversa e sugere ajuste.
- **Pós-processo local e determinístico** [O]:
  - substituições do dicionário e snippets;
  - minúsculas;
  - comando falado **"press enter"**, que remove a frase e tecla Enter
    depois de colar (confirmação na primeira vez);
  - assinatura de e-mail;
  - "at Fulano" → `@Fulano` no Slack;
  - "stacked messages" divide o ditado em várias mensagens de chat.
- **Transform / Polish** [O]: atalho global que pega a seleção (via UI
  Automation, ou simulando Ctrl+C), manda ao LLM com uma instrução padrão
  ou um dos 9 prompts salvos (com amostras de escrita do usuário) e
  substitui a seleção colando. Tem diff, Undo detectado e "auto-polish
  depois do ditado" opcional.
- **Modo comando "Hey Flow" / Instruct** [O]: canal realtime separado
  (WebSocket, PCM 24 kHz) com ferramentas que o modelo pode chamar:
  - gerar/editar texto;
  - aplicar transform;
  - mudar configurações por voz;
  - consultar histórico local, com limites;
  - colar e mostrar no chat.

  No máximo 12 chamadas por sessão. `textRoutingEligibility` é **só o
  portão** desse canal (flag + modo + modelo permitido), **não** um
  roteamento por app.
- **Fora do escopo do Vyrp v1**: gravador de reuniões, notas e scratchpad,
  "Lens", extensões e MCP, times, cobrança.

### 1.6 Dicionário e snippets
- **Armazenamento** [O]: uma tabela só para os dois.
  - Vocabulário = frase sem substituição; serve para o ASR/LLM grafar certo.
  - Substituição = frase → texto.
  - Snippet = gatilho → expansão, em texto e opcionalmente HTML.
- **Aplicação** [O]: uma **regex compilada única** (sem distinção de caixa,
  com fronteiras de palavra Unicode). Se a fala **inteira** é o gatilho de
  um snippet, sai só a expansão.
- **Primeiro uso** [O]: o app semeia exemplos no dicionário.
- **Auto-aprendizado** [O], a partir de edições do usuário:
  1. Depois de colar, o nativo **observa o campo** e devolve o texto
     editado.
  2. O app alinha palavra a palavra o texto colado e o editado.
  3. Extrai até 4 candidatos de até 4 palavras (filtra stopwords e o que já
     existe).
  4. Um classificador no servidor confirma quais são termos novos.
  5. O termo é gravado junto com o **erro original do ASR**.

### 1.7 Contexto do app em foco
- **O que é capturado no início do ditado e na troca de foco** [O]:
  - app (exe/nome, URL quando é navegador, tipo: e-mail/IA/terminal/outro);
  - **texto antes, seleção e texto depois do cursor**, truncados em
    fronteira de palavra;
  - "o campo é editável?";
  - textos próximos (thread/conversa);
  - em IDEs, nomes de arquivos e de variáveis.
- **Nomes próprios da tela** [O]: extraídos por uma chamada leve de LLM
  **em paralelo** com a fala (≤ ~140 caracteres); OCR opcional.
- **Exclusões** [O]: campo de senha não é lido. Em produção o texto de tela
  **não é persistido**; isso só acontece com uma flag de dev.
- **Política de dados** [O]: guardar normalmente, apagar em 24 h, ou nunca
  guardar.
- **Deny-list** de apps e URLs [O]: checada no início do ditado e de novo
  antes de colar.

### 1.8 Inserção do texto
- **Método principal** [O]: clipboard + **Ctrl+V sintético** (SendInput)
  feito pelo nativo. **Shift+Insert** em terminais. Textos longos em CLIs de
  código são colados **em partes**.
- **Clipboard** [O]:
  - **snapshot completo** (todos os formatos) antes de colar;
  - **restauração** ~50 ms depois, cancelável, que pula se o clipboard mudou
    nesse meio-tempo;
  - publicação com **renderização atrasada** numa thread STA;
  - marcação para **não entrar no histórico/nuvem** do clipboard do Windows;
  - `OpenClipboard` com retry.
- **Antes de colar** [O]: espera os modificadores serem soltos. Há
  "mascaramento" (uma tecla inócua) para Alt/Win não abrirem menu/Iniciar, e
  o toggle do Caps Lock é limpo quando o atalho é de tecla única.
- **Confirmação** [O]:
  - cada paste tem id e devolve desfecho, com tempo e motivo (por exemplo,
    **UIPI**: janela elevada);
  - **verificação** via UI Automation de que o texto aterrissou, comparando
    antes e depois do cursor;
  - detecção de "colou o clipboard **antigo**".
  - Em falha: aviso com botão copiar e atalhos "colar último" / "copiar
    último".
- **Sem campo editável** [O]: notificação com o texto e botão copiar.

### 1.9 Histórico, onboarding, settings, idiomas
- **Histórico** [O]: por ditado, guarda:
  - texto cru, formatado, colado e editado pelo usuário;
  - app/URL, idioma, duração, palavras;
  - latências;
  - áudio opcional.

  Ações: copiar, recolar, desfazer a IA (voltar ao cru) e busca.
- **Onboarding** [O]:
  1. Permissões (mic, acessibilidade, captura de tela opcional).
  2. Escolha/quiz do microfone.
  3. Tutorial do atalho.
  4. "Experimente" em apps simulados.
  5. Perguntas de perfil.
  6. Estilo por categoria.
  7. Teste num app real.

  Há atalho para "onboarding rápido".
- **Settings** [O]:
  - geral: idiomas, mic, atalhos, idioma da UI;
  - sistema: sons, mídia, auto-dicionário, notificações por categoria;
  - formatação e limpeza;
  - integração com IDEs;
  - dados e privacidade;
  - experimental;
  - conta.
- **Notificações** [O]: ~200 tipos, cada uma com contador e data da última
  exibição (rate-limit).

---

## 2. Arquitetura do Wispr Flow (Windows)

| Processo / módulo | Função |
|---|---|
| **Electron main** | Cérebro: máquina de estados do ditado, decisão de atalho, rede (gRPC/HTTP/WebSocket), prefs (`electron-store`), SQLite (`sqlite3` + Sequelize/umzug), feature flags (PostHog), telemetria (Sentry). [O] |
| **Renderer "hub" (oculto)** | Captura de áudio: Web Audio, AudioWorklet, WebCodecs Opus. [O] |
| **Renderers visíveis** | Overlay/flow bar e janela principal (React). [O] |
| **`Wispr Flow Helper.exe`** | .NET **Native AOT** (C#, ~20 MB, sem runtime). Hook **WH_KEYBOARD_LL** e **WH_MOUSE_LL** com loop próprio; `GetAsyncKeyState` para reconciliar teclas presas; **UI Automation** (Text/Value/LegacyIAccessible pattern, TreeWalker, CacheRequest); `SetWinEventHook` para foco e seleção; **clipboard** e **SendInput**; monitor WASAPI (quem segura o mic, privacidade, fade/mute de mídia, sons). **Não captura áudio de ditado.** [O] |
| **`.node` addons** | Não têm lógica de ditado: `win-ca` (raízes de certificados do Windows) e `node_sqlite3`. [O] |
| **`jabra-device-connector.exe`** | SDK de terceiro, só para wear-detection, opt-in. [O] |
| **Workers JS** | Opus para salvar arquivos (histórico/reuniões), profiling, extensões. [O] |

**IPC main ↔ helper** [O]:
- **Transporte**: spawn com 4 pipes. stdin = main → helper; fd 3 = helper → main.
- **Formato**: JSON UTF-8 delimitado por `|`, com escape simples; até 100 mil
  caracteres por mensagem.
- **Envelope**: request/response com uuid, **nos dois sentidos**; timeout
  de 3 s.
- **Saúde do helper**:
  - keepalive de 500 ms;
  - ping depois de 2 s de silêncio; relança depois de 15 s;
  - backoff de relaunch 0–16 s;
  - fila de escrita com backpressure.
- **Morte do pai**: o helper sai junto.
- **Watchdog do hook**: o Windows desliga hooks lentos. Uma "tempestade" de
  desligamentos faz o helper sair com um código dedicado, e o main o relança.
- **Mensagens** (~100 tipos), por grupo: ciclo do ditado; foco/contexto (guardar
  e restaurar o elemento focado, ler o campo); teclado (fluxo cru,
  teclas presas, simular tecla); paste e desfecho/verificação/edição
  observada; mic/áudio; flags.

**VAD:** nenhum, nem no cliente nem no nativo [O]. Corte e pontuação ficam no servidor [I].

---

## 3. Esquema de dados (`flow.sqlite` e config)

### 3.1 Tabelas relevantes para ditado (colunas resumidas)
- **History** (PK uuid por ditado). As colunas formam grupos:
  - **texto**: cru do ASR, formatado, colado, editado pelo usuário, versões
    "padrão" vs "fallback";
  - **contexto**: app, URL, conteúdo do campo, texto de acessibilidade,
    comando usado (ptt/popo/instruct), estilo aplicado, idioma
    pedido/detectado, confiança;
  - **áudio**: blob WAV, blocos Opus, mic usado;
  - **métricas**: latência e2e (liberar a tecla → colado), latência de rede
    do cliente, duração da fala, palavras, palavras corrigidas,
    substituições do dicionário, distância de edição, nível de fallback;
  - **aprendizado**: status e tentativas da observação de edição, motivo do
    fim da observação, "reverteu a IA";
  - **sync**: `needsUploading`, `isArchived`.
- **Dictionary**: frase, substituição (texto e HTML), `isSnippet`,
  `manualEntry`, origem e "erro observado do ASR", frequência local/remota,
  último uso, favorito, soft delete, time.
- **Polish**: texto inicial e polido, instrução ou nome do prompt, app,
  modelo, provedor, tempo, "desfeito", feedback.
- **UserContext**: amostras de escrita e prompts.
- **UsageOutbox**: fila de métricas com tentativas.
- **Instruct\***: roteamento do modo comando.
- **Fora do escopo do Vyrp**: Meetings, Notes, Todos, Calendar, Folders,
  Lens, Notetaker, importação de terceiros.

**Padrões** [O]:
- toda tabela sincronizável tem `needsUploading` indexado;
- soft delete com `isDeleted`;
- JSON guardado em colunas TEXT;
- migrations versionadas: 59 arquivos na pasta, mais outras embutidas no
  bundle.

### 3.2 `config.json` (store de preferências)
- **`prefs.user`** (~250 chaves). As úteis como referência:
  - **atalhos**: mapa "VKs unidos por `+`" → ação, mais um "modificador"
    padrão;
  - **áudio**: mic forçado, lista ranqueada de mics, mutar/pausar mídia,
    sons;
  - **formatação**: formatação IA ligada, formato padrão e por app, nível
    de limpeza, estilo por categoria, "voices" (categoria → apps, limpeza,
    estilo), detecção de estilo, comando "press enter";
  - **polish**: instruções padrão e custom, auto-polish;
  - **contexto**: usar contexto de acessibilidade, OCR, integração IDE,
    focus mode (apps/URLs bloqueados);
  - **idiomas**: selecionados, usados, efetivos, idioma da UI;
  - **dicionário**: auto-aprender, ordenação;
  - **UI**: ocultar barra, borda do dock, tema, abrir no login;
  - **privacidade**: política de dados local, sync na nuvem, compartilhar
    uso;
  - **permissões**: microfone, acessibilidade e captura de tela, cada uma
    com "clicou" e "concedida".
- **`notifications`**: tipo → {última exibição, vezes exibida}.
- **`degradedNetwork`**: detecção de rede ruim, com a última entrega gRPC.
- **`feature-flags-cache.json`**: ~455 flags (PostHog), anônimas e
  identificadas. Ex.: corrida gRPC×HTTP, timeouts gRPC, desligar a
  restauração do clipboard, aviso de paste falho, reabrir o mic em
  "silent start", aprendizado de frases.
- **Sessão/token**: não copiados, por política.

---

## 4. Latência e truques de UX que valem como ideia

**Latência (caminho crítico = liberar a tecla → texto colado)**
1. **Pré-aquecer tudo antes do gesto.** O grafo de áudio está pronto, o
   modelo e a conexão estão quentes, e o contexto é coletado *enquanto* a
   pessoa fala. No Vyrp: o modelo whisper fica carregado na GPU desde o
   boot.
2. **Abrir o stream de STT no pressionar** e mandar áudio incremental. Só
   o último pedaço e o commit ficam no caminho crítico.
3. **Fila de 1 slot que concatena** em vez de timers de envio.
4. **Corrida com plano B** só para áudio curto, com descarte do perdedor por
   id, e **heartbeat com "segundos recebidos"** para detectar atraso cedo.
5. **Contexto fora do caminho do áudio**: o *init* leva dicionário e estilo;
   o *context* pode ser reenviado no meio da fala.
6. **Desligar AEC/NS/AGC**; 16 kHz mono int16 como base.
7. **Descartar blocos zerados de aquecimento** e reaproveitar um mic que
   acabou de abrir.
8. **Medir cada fase** (primeiro bloco, stop→commit, RTT, servidor por
   etapa, paste) e guardar por ditado.

**UX**
1. Duplo toque no PTT trava o hands-free; `Esc` cancela; debounce de 500 ms;
   aviso antes do limite.
2. Estados explícitos no overlay, incluindo "processando" e "limpeza
   concluída, ver diff", com Undo.
3. Paste confirmado: id, verificação, "colou o clipboard antigo" e fallback
   visível com copiar; atalhos colar/copiar último.
4. Clipboard restaurado e fora do histórico; mascarar Win/Alt; limpar Caps
   Lock.
5. Estilo por **categoria** de app (poucas opções, com exemplo) e níveis de
   limpeza com antes/depois.
6. Dicionário e snippets juntos; "fala inteira = gatilho"; aprendizado a
   partir das edições, guardando o erro do ASR.
7. Texto de tela nunca persistido por padrão; política "nunca guardar".
8. Deny-list checada duas vezes; chunking de paste para terminais.
9. Notificações com rate-limit por tipo; catálogo de erros de mic/rede bem
   nomeado.
10. "Press enter" falado, com confirmação na primeira vez.

---

## 5. Arquitetura proposta para o Vyrp

**Decisões do usuário (fixas):**
- Tauri 2 + Rust + React/TS.
- STT local com **whisper.cpp** (`whisper-rs`), com GPU quando houver.
- STT na nuvem **opcional** (Groq / OpenAI).
- Limpeza por LLM **opcional**, com a chave do próprio usuário e provedor
  plugável (Claude / OpenAI / Ollama).

### 5.1 Visão geral
```
┌──────────────── processo Vyrp (Rust, tokio) ───────────────────────────┐
│ hotkey (thread hook LL) ─► gesture FSM ─► dictation orchestrator        │
│ audio (cpal/WASAPI thread) ─► ring buffer 16k mono ─► VAD               │
│ context (thread STA UIA)        stt: Local(whisper) | Groq | OpenAI     │
│ insert (thread STA clipboard)   post: dicionário/snippets ► LLM opc.    │
│ storage (SQLite) · settings · secrets (Credential Manager) · metrics    │
└───────────────▲─────────────── eventos/commands Tauri ─────────────────┘
                │
   webview "overlay" (transparente, sem foco)   webview "hub" (settings, histórico, dicionário, onboarding)
```

**Escolhas e o porquê:**
- **Um processo só, com threads dedicadas**, em vez de um helper separado.
  O Rust já é nativo; o helper do Wispr existe porque o Electron não chega
  ao Win32. Mitigações no lugar do processo separado:
  - **thread própria do hook**, com `GetMessage` e callback mínimo (só
    enfileira);
  - **watchdog** que reinstala o hook;
  - UIA e clipboard em **threads STA próprias**, com timeouts.
  - Se o hook se mostrar instável, a extração para um processo vira um card
    à parte (V-X1).
- **Áudio nativo** (`cpal` sobre WASAPI) em vez de Web Audio: menos
  latência, e o buffer fica perto do whisper.
  - **Pre-roll** (melhoria sobre o Wispr): com o modo "mic quente" ligado,
    um ring buffer de ~300 ms fica aberto, e a primeira sílaba não se perde.
    Desligado por padrão, por privacidade; o indicador mostra.
- **VAD local** (Silero via `ort`, ou `webrtc-vad` como plano B): corta
  silêncio antes do whisper, o que reduz alucinação e tempo, e marca pausas
  para a decodificação incremental.
- **STT local primeiro**: modelo carregado na GPU no boot (CUDA ou Vulkan
  como features de build; CPU como fallback). O caminho crítico vira
  "decodificar o áudio todo ao soltar":
  - **decodificação incremental**: a cada pausa detectada pelo VAD durante a
    fala, decodifica o segmento fechado em background; ao soltar, só o último
    segmento fica no caminho crítico;
  - o prompt inicial do whisper recebe o vocabulário do dicionário.
- **STT na nuvem**: atrás do mesmo trait. Upload ao soltar (Groq é rápido o
  bastante para dispensar streaming na v1). **Política** configurável: só
  local, só nuvem, ou nuvem com fallback local por timeout.
- **LLM opcional**: trait `LlmProvider` (Claude, OpenAI, Ollama).
  - Prompts **próprios**, por nível de limpeza e estilo de categoria.
  - Entradas: texto cru, categoria do app, vocabulário, até N caracteres
    antes/depois do cursor.
  - **Timeout curto**: estoura → cola o texto sem LLM e avisa no overlay.
  - Desligado = só o pós-processo determinístico.
- **Inserção**: clipboard + Ctrl+V com snapshot/restauração e exclusão do
  histórico; Shift+Insert em terminais.
  - Fallback **SendInput Unicode** (digitação) para texto curto, ou quando o
    clipboard falha.
  - Verificação via UIA e "colar/copiar último".
- **Dados**: SQLite (`rusqlite`) com migrations; settings tipados em JSON;
  **chaves de API no Windows Credential Manager** (`keyring`), nunca no JSON
  nem no SQLite.

### 5.2 Contratos fixados antes dos cards (interface entre módulos)
Vivem em `src-tauri/src/core/types.rs`, criado no V2. Quem muda um contrato
avisa o Cérebro.
- `KeyEvent { vk: u16, down: bool, injected: bool, t: Instant }`
  (canal hook → FSM).
- `GestureEvent = PttStart | PttEnd | LockStart | LockEnd | Cancel |
  Action(ActionId)` (FSM → orchestrator).
- `DictationState = Idle | Arming | Listening{locked} | Processing | Inserting
  | Done | Error(code)`. É emitido como evento Tauri `dictation://state` com
  nível de áudio em `dictation://level`.
- `AudioClip { pcm: Vec<i16>, sample_rate: 16000, speech_segments:
  Vec<Range<usize>> }`.
- `trait SttEngine { async fn transcribe(&self, clip: &AudioClip, opts:
  &SttOptions) -> Result<Transcript> }`, com `Transcript { text, language,
  segments, timings }`. `SttOptions` leva idiomas e vocabulário.
- `AppContext { exe, title, url: Option, category: AppCategory, editable:
  bool, before: String, selection: String, after: String }`.
- `trait LlmProvider { async fn clean(&self, req: &CleanRequest) ->
  Result<String> }`; `CleanRequest { raw, level, style, context: AppContext,
  vocabulary }`.
- `InsertRequest { text, html: Option, method: Auto|Paste|ShiftInsert|Type,
  press_enter: bool }`; o resultado é
  `InsertOutcome { ok, method, verified: Option<bool>, reason }`.
- **Marcos de latência**: `t_press, t_first_audio, t_release, t_stt_done,
  t_llm_done, t_inserted`, gravados por ditado.

### 5.3 Metas de latência (liberar → colado, ditado de 10 s)
| Caminho | Meta |
|---|---|
| Local GPU, `small`/`base`, sem LLM | ≤ 600 ms |
| Local GPU + LLM nuvem | ≤ 1,5 s |
| Groq, sem LLM | ≤ 900 ms |

Acima de 4 s o overlay mostra "demorando".

---

## 6. Cards de implementação

Formato: **id — título** · dono sugerido · *entrada* → *saída* · deps.
Lead = julgamento/Win32/arquitetura; Prog = spec fechada.

Fases:
- **F0**: base.
- **F1**: ditado local ponta a ponta (MVP).
- **F2**: nuvem e LLM.
- **F3**: polimento de produto.
- **F4**: extras.

O caminho crítico do MVP é **V2 → V5 → V6 → V7 → V9 → V11 → V15**. Os
demais cards de F1 correm em paralelo.

### F0 — Base
- **V2 — Scaffold Tauri 2** · Lead
  - *Entrada*: repo vazio.
  - *Saída*:
    - workspace Tauri 2 + React/TS (Vite) + Rust;
    - módulos vazios por área (`hotkey`, `audio`, `vad`, `stt`, `context`,
      `post`, `llm`, `insert`, `storage`, `settings`, `metrics`);
    - `core/types.rs` com os contratos do §5.2;
    - logging `tracing` em arquivo, single-instance, tray com sair/abrir;
    - duas janelas: `hub` normal e `overlay` transparente, sempre no topo,
      sem foco e sem barra de tarefas;
    - scripts lint/test/build;
    - **`.evidencias.json`** (captura de janela do Vyrp).
  - *Deps*: —
- **V3 — Settings + segredos** · Prog
  - *Entrada*: V2.
  - *Saída*:
    - struct `Settings` (serde, com defaults e versão) persistida em JSON
      no AppData;
    - commands `get_settings`/`set_settings` e evento de mudança;
    - `keyring` para as chaves Groq/OpenAI/Anthropic, com commands
      set/clear/has (o valor nunca volta à UI);
    - testes de default e migração de versão.
  - *Deps*: V2.
- **V4 — SQLite + migrations** · Prog
  - *Entrada*: V2.
  - *Saída*:
    - `rusqlite` com migrations versionadas;
    - tabelas `history` (texto cru/limpo/inserido, app, categoria, idioma,
      engine, provedor LLM, duração, palavras, marcos de latência, status,
      áudio opcional), `dictionary` (frase, substituição, html, is_snippet,
      origem, erro_observado, freq, último uso, favorito, deleted) e
      `kv_notifications`;
    - repositórios com testes;
    - política de retenção (guardar / 24 h / nunca).
  - *Deps*: V2.

### F1 — Ditado local ponta a ponta
- **V5 — Hook de teclado** · Lead
  - *Entrada*: V2.
  - *Saída*:
    - thread com `WH_KEYBOARD_LL` (+ `WH_MOUSE_LL` para os botões 4/5) e
      callback mínimo, que publica `KeyEvent` num canal;
    - reconciliação de teclas presas via `GetAsyncKeyState`;
    - **watchdog** que reinstala o hook se ele parar de receber eventos
      (teste de "tempestade");
    - supressão opcional da tecla quando faz parte de um atalho;
    - **mascaramento** de Win/Alt (tecla inócua) e limpeza do Caps Lock em
      atalho de tecla única;
    - ignora eventos `injected` do próprio app.
  - *Deps*: V2.
- **V6 — Máquina de gestos** · Lead
  - *Entrada*: V5, V3.
  - *Saída*: FSM pura e testável (sem Win32) que transforma `KeyEvent` em
    `GestureEvent`:
    - PTT por segurar, com hold mínimo e debounce de 500 ms;
    - **duplo toque = trava hands-free** (janela configurável);
    - `Esc` = cancelar;
    - chords de até 3 teclas, com distinção esquerda/direita;
    - **checagem de chord fantasma** (pergunta ao V5 o estado físico);
    - validação de conflitos;
    - testes de tabela cobrindo as sequências.
  - *Deps*: V5, V3.
- **V7 — Captura de áudio** · Lead
  - *Entrada*: V2, V3.
  - *Saída*:
    - `cpal`/WASAPI no mic escolhido, com lista ranqueada e fallback para o
      padrão do SO;
    - reamostragem para 16 kHz mono i16 (`rubato`);
    - stream **pré-aquecido** (aberto e pausado), ou "mic quente" com
      **pre-roll** de 300 ms (configurável);
    - evento de nível (~30 Hz) para o overlay;
    - avisos de silêncio total (5 s / 15 s), de mic desconectado e de mic
      novo;
    - limite de 20 min, com aviso aos 19 min;
    - produz `AudioClip`;
    - teste com WAV sintético.
  - *Deps*: V2, V3.
- **V8 — VAD** · Prog (spec: Silero ONNX via `ort`; plano B `webrtc-vad`)
  - *Entrada*: V7.
  - *Saída*:
    - `speech_segments` no `AudioClip`;
    - trim do silêncio inicial e final;
    - evento "pausa detectada" para a decodificação incremental;
    - "sem fala" detectado sem chamar o STT;
    - testes com WAVs de fixture.
  - *Deps*: V7.
- **V9 — STT local (whisper)** · Lead
  - *Entrada*: V7 (V8 opcional).
  - *Saída*:
    - `WhisperEngine: SttEngine` via `whisper-rs`;
    - features de build `cuda` / `vulkan` / CPU, com detecção de GPU em
      runtime;
    - **modelo carregado no boot** e mantido em memória;
    - **gerenciador de modelos**: catálogo (tiny/base/small/medium/
      large-v3-turbo, com tamanho e idioma), download com progresso e
      SHA-256, pasta no AppData;
    - prompt inicial com vocabulário;
    - idioma fixo ou auto;
    - **decodificação incremental** por segmento do VAD;
    - benchmark em CLI (`cargo run --bin bench-stt`) que imprime a latência
      por modelo.
  - *Deps*: V7 (V8 para o incremental).
- **V11 — Orquestrador do ditado** · Lead
  - *Entrada*: V6, V7, V9, V12, V14, V15.
  - *Saída*:
    - `DictationState` e eventos;
    - **no pressionar**, em paralelo: inicia a captura e o contexto, e
      mostra o overlay;
    - **no soltar**: fecha o clip → STT → pós-processo → [LLM] →
      inserir → gravar histórico;
    - cancelamento em qualquer fase;
    - ditado curto demais ou vazio é descartado sem colar;
    - marcos de latência gravados;
    - erros mapeados para códigos/notificações;
    - **teste ponta a ponta** com STT falso.
  - *Deps*: V6, V7, V9, V12, V14, V15 (V4 para o histórico).
- **V12 — Contexto do app em foco** · Lead
  - *Entrada*: V2.
  - *Saída*:
    - thread STA com `IUIAutomation`;
    - `GetForegroundWindow` → exe e título;
    - elemento focado: editável? senha? Texto antes/seleção/depois via
      TextPattern/ValuePattern/LegacyIAccessible, truncado em fronteira de
      palavra (limite configurável);
    - URL best-effort em Chrome/Edge/Firefox;
    - categoria do app por tabela editável (trabalho/e-mail/pessoal/outros);
    - timeout duro de 150 ms; falha → contexto vazio, nunca bloqueia.
  - *Deps*: V2.
- **V14 — Pós-processo determinístico** · Prog
  - *Entrada*: V4.
  - *Saída*:
    - dicionário e snippets numa **regex compilada única** (sem distinção
      de caixa, fronteiras Unicode), recompilada quando o dicionário muda;
    - "fala inteira = gatilho" → expansão;
    - comando **"aperte enter"/"press enter"** no fim → remove e marca
      `press_enter`;
    - formatos por app: cru / minúsculo / normal;
    - espaço inicial conforme o texto antes do cursor;
    - testes de tabela.
  - *Deps*: V4.
- **V15 — Inserção de texto** · Lead
  - *Entrada*: V5, V12.
  - *Saída*:
    - snapshot de **todos os formatos** do clipboard e restauração depois
      de ~50–150 ms (cancelada se o clipboard mudou);
    - formatos de exclusão do histórico e da nuvem;
    - `OpenClipboard` com retry;
    - espera os modificadores físicos serem soltos;
    - Ctrl+V via SendInput; Shift+Insert em terminais (lista de exe);
      fallback **SendInput Unicode**;
    - texto + HTML opcional;
    - `InsertOutcome` com verificação UIA (texto apareceu?), incluindo
      detecção de UIPI (janela elevada);
    - "colar último" / "copiar último";
    - matriz de teste manual (Notepad, Chrome, Word, Slack, VS Code,
      Windows Terminal).
  - *Deps*: V5, V12.
- **V16 — Overlay** · Prog (spec de estados abaixo)
  - *Entrada*: V2, eventos do V11.
  - *Saída*:
    - React na janela `overlay`, com estados repouso / ouvindo (PTT) /
      ouvindo (travado) / processando / erro / "copiado, cole manualmente";
    - waveform pelo nível;
    - na borda inferior central, arrastável, ocultável;
    - nunca rouba foco;
    - som de início/fim (V19 pode vir depois).
  - *Deps*: V2, V11 (eventos podem ser simulados).
- **V17a — Hub mínimo** · Prog
  - *Entrada*: V3, V4.
  - *Saída*:
    - telas Geral (mic, atalho com **gravador de atalho**, idioma, modelo
      whisper com download) e Histórico (lista, copiar, recolar, cru vs
      final, busca);
    - tudo em PT-BR, via i18n desde o início.
  - *Deps*: V3, V4 (e V9 para o download de modelo).

### F2 — Nuvem e LLM (opcionais)
- **V10 — STT na nuvem** · Prog
  - *Entrada*: trait do V9, V3.
  - *Saída*:
    - `GroqEngine` e `OpenAiEngine: SttEngine` (upload WAV/FLAC no soltar,
      com idioma e prompt);
    - timeouts;
    - **política**: só local / só nuvem / nuvem com fallback local
      (timeout configurável, 3 s por padrão);
    - teste com servidor HTTP falso.
  - *Deps*: V9 (trait), V3.
- **V13 — Limpeza por LLM** · Lead
  - *Entrada*: V3, V12.
  - *Saída*:
    - `LlmProvider` com Claude, OpenAI e Ollama (endpoint local);
    - **prompts próprios** por nível (nenhum/leve/médio/alto) × estilo
      (formal/casual/muito casual);
    - entradas: cru + categoria + vocabulário + contexto truncado;
    - saída só o texto, com guarda contra resposta conversacional (o
      modelo "respondendo" ao ditado vira fallback);
    - timeout curto (padrão 2,5 s) → texto sem LLM;
    - testes com respostas gravadas.
  - *Deps*: V3, V12.
- **V17b — Hub: provedores e estilos** · Prog
  - *Entrada*: V10, V13.
  - *Saída*:
    - tela de STT (local/nuvem/política) e de LLM (provedor, modelo, chave
      via keyring, "testar");
    - estilo por categoria com exemplo antes/depois;
    - nível de limpeza;
    - tabela app → categoria.
  - *Deps*: V10, V13.
- **V18 — Dicionário e snippets (UI)** · Prog
  - *Entrada*: V4, V14.
  - *Saída*:
    - CRUD com busca, favoritos e import/export CSV;
    - snippets com gatilho;
    - seed de exemplos no primeiro uso.
  - *Deps*: V4, V14.

### F3 — Produto
- **V19 — Sons e mídia** · Prog
  - *Entrada*: V11.
  - *Saída*:
    - sons de início, fim e erro (assets próprios);
    - pausar mídia (tecla de mídia / sessão SMTC) ou silenciar o volume
      durante o ditado, com restauração garantida;
    - opções nas settings.
  - *Deps*: V11.
- **V20 — Onboarding** · Prog
  - *Entrada*: V17a, V9.
  - *Saída*:
    - checar a permissão de microfone do Windows (privacidade);
    - teste de mic com medidor;
    - escolher e baixar o modelo (sugestão pela GPU);
    - tutorial do atalho;
    - campo "experimente aqui";
    - provedores opcionais.
  - *Deps*: V17a, V9.
- **V21 — Deny-list e privacidade** · Prog
  - *Entrada*: V4, V12.
  - *Saída*:
    - lista de apps/URLs bloqueados, checada no início e antes de colar;
    - retenção de histórico (guardar / 24 h / nunca);
    - áudio do histórico opcional (desligado por padrão);
    - contexto de tela nunca persistido.
  - *Deps*: V4, V12.
- **V22 — Métricas e diagnóstico** · Prog
  - *Entrada*: V11.
  - *Saída*:
    - painel com latência por fase dos últimos N ditados (p50/p95);
    - exportar log;
    - "copiar diagnóstico" sem texto do usuário.
  - *Deps*: V11.
- **V23 — Empacotamento e atualização** · Lead
  - *Entrada*: tudo de F1.
  - *Saída*:
    - instalador NSIS/MSI (bundler Tauri), com variantes de build
      CPU/CUDA/Vulkan ou detecção em runtime;
    - abrir no login;
    - `tauri-plugin-updater`;
    - nota sobre assinatura de código e SmartScreen.
  - *Deps*: F1.

### F4 — Extras (depois do MVP)
- **V24 — Auto-aprendizado do dicionário** · Lead
  - *Entrada*: V12, V15, V4.
  - *Saída*:
    - após colar, observar o campo via UIA (eventos de mudança de texto,
      até perder foco ou ~15 s);
    - diff palavra a palavra;
    - até 4 candidatos de até 4 palavras (filtro de stopwords PT/EN e do
      que já existe; classificação opcional por LLM);
    - grava com o erro do ASR;
    - notificação com desfazer.
  - *Deps*: V12, V15, V4.
- **V25 — Transform da seleção** · Lead
  - *Entrada*: V13, V15, V12.
  - *Saída*:
    - atalho → seleção (UIA ou Ctrl+C simulado, com restauração do
      clipboard) → LLM com instrução padrão ou prompt salvo (1..9) →
      substitui;
    - diff e Undo;
    - histórico próprio.
  - *Deps*: V13, V15, V12.
- **V26 — Comandos de voz simples** · Prog
  - *Entrada*: V14.
  - *Saída*:
    - "nova linha", "novo parágrafo" e "apagar isso" (desfaz o último
      paste);
    - lista configurável PT/EN.
  - *Deps*: V14.
- **V-X1 — Extrair hook/UIA para processo helper** · Lead · *só se* V5/V12
  mostrarem instabilidade em campo
  - *Saída*: protocolo JSON por pipe, keepalive e relaunch (modelo do §2).

**Dependências em uma linha:**
- V2 → {V3, V4, V5, V7, V12};
- V5 → V6;
- V7 → {V8, V9};
- V4 → V14;
- {V5, V12} → V15;
- {V6, V7, V9, V12, V14, V15} → V11;
- V11 → {V16 (pode começar antes, com eventos simulados), V19, V22};
- {V3, V4} → V17a;
- V9 → V10;
- {V3, V12} → V13;
- {V10, V13} → V17b;
- {V4, V14} → V18;
- F1 → V23;
- F4 depois de F1/F2.

**Paralelismo sugerido** (2 leads + 1 programmer): depois do V2, o Lead A
pega V5 → V6 → V15, o Lead B pega V7 → V9 → V11 e o Prog pega V3 → V4 →
V14 → V8.

---

## 7. Desconhecidos (não bloqueiam o Vyrp)
- Modelos e prompts do servidor do Wispr, critério do "ASR externo",
  algoritmo do "tone match" e do "divergence score": ficam no servidor.
- Tempos exatos de hold mínimo e da janela de duplo toque (constantes não
  extraídas). O Vyrp define os seus no V6, com padrões de 150 ms e 350 ms,
  ajustáveis.
- Fluxo do "Lens", e uso exato do loopback WASAPI e do "mic ring" BLE no
  helper.
- O helper Native AOT não foi desmontado: o que está no §2 vem de
  metadados, strings e do lado JS do protocolo.
