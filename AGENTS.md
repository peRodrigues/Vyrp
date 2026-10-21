# AGENTS.md — Vyrp

> Contexto compartilhado para assistentes de IA (Claude Code, Codex e afins).
> **Este arquivo é um roteador, não a documentação.** Mantenha-o curto; o detalhe
> vive nos docs da §9. Em português. O `CLAUDE.md` só importa este arquivo.

## 1. O que é

Ditado por voz para Windows: o usuário segura um atalho, fala, solta, e o texto
limpo é colado no app em foco.

Decisões de direção (não mude sem falar com o Cérebro):
- **Tauri 2 + Rust + React/TS (Vite).** Processo único, threads por área.
- **STT local primeiro**: whisper.cpp (`whisper-rs`) com modelos baixados sob
  demanda; **nuvem opcional** (Groq/OpenAI) com política local / nuvem / nuvem com reserva local.
- **Limpeza por LLM opcional** (Anthropic, OpenAI, Ollama), com a **chave do próprio
  usuário** no Credential Manager. Sem LLM, vale só o pós-processo determinístico.
- **Clean-room**: o `docs/re/wispr-flow-spec.md` descreve comportamento observável;
  **nada do Wispr é copiado** (código, prompts, textos, sons, ícones). Prompts, sons e
  textos são nossos.
- Strings da UI via `src/hub/i18n.ts` (`t()`), em `lang/pt-BR.ts` e `lang/en.ts` (mesmas chaves); nunca texto solto no JSX novo.
- Mensagem do Rust que chega à tela é `UiError` (`core/ui_error.rs`): código estável + parâmetros, `Display` em português só para o log. A UI traduz por `uierr.<código>` (`uiErrorText`); código novo no Rust exige a chave nos dois idiomas (o teste de paridade confere). Texto do tray: `tray_i18n.rs`.

## 2. Mapa dos módulos

`src-tauri/src/` (Rust):

| Módulo | O que é |
|---|---|
| `core/` | contratos entre módulos (`types.rs`) — ver §3 |
| `hotkey/` | hook de teclado/mouse (V5) e máquina de gestos (V6) |
| `audio/` | captura cpal/WASAPI → mono 16 kHz, medidor de nível, avisos de mic |
| `vad/` | detecção de fala (Silero) e descarte de clipe sem fala |
| `stt/` | `SttEngine`: whisper local (`whisper.rs`), modelos/catálogo, nuvem (`cloud.rs`) + política |
| `post/` | pós-processo determinístico: dicionário, snippets, "aperte enter", comandos de voz |
| `llm/` | limpeza por LLM (`LlmProvider`), prompts próprios, guarda anti-resposta, fallback p/ texto cru |
| `context/` | contexto do app em foco (V12) |
| `insert/` | inserção do texto no app em foco (V15) |
| `dictation/` | orquestrador do ditado (V11): gestos → áudio → STT → pós → LLM → inserir |
| `factory.rs` | monta STT/LLM a partir do Settings, a cada ditado |
| `settings/` | Settings em JSON versionado (migrações) + chaves de API (`secrets`, Credential Manager) |
| `storage/` | SQLite: histórico, dicionário, notificações, migrations |
| `sound/` | sons sintetizados + mídia (pausar/silenciar/baixar) com restauração garantida |
| `overlay.rs` | janela overlay clicável e simulador de estados |
| `hub.rs` | commands só do hub: histórico, microfones, testes de áudio, testar provedor |
| `metrics/` | marcos de latência |
| `lib.rs` | registro de módulos, commands e `setup` (**só acrescente**; várias frentes mexem aqui) |

`src/` (React/TS): `hub/` (telas: Início, Geral, Provedores, Histórico, Dicionário,
Avançado, Onboarding; `*Api.ts` espelham commands; regras puras em `*State.ts`/`*Rules.ts`
com teste), `overlay/` (barra de ditado), `shared/` (estados do ditado, preferência de ocultar).

Sufixo de nome: no Windows `Foo.tsx` e `foo.ts` colidem — dê nomes diferentes.

## 3. Contratos (`src-tauri/src/core/types.rs`)

`SttEngine`, `LlmProvider`, `CleanRequest`, `AudioClip`, `DictationState`, eventos
`dictation://state` e `dictation://level`, `AppContext`, `InsertRequest`, `KeyEvent`/`GestureEvent`.
**Quem mudar um tipo daqui muda a interface entre cards: avise o Cérebro antes.**
Eventos/commands do hub com a UI: `stt/mod.rs` (modelos), `settings://changed`.

## 4. Build

Requer Rust MSVC + VS Build Tools, Node 24+, **CMake** e **libclang** (whisper.cpp):
- `LIBCLANG_PATH` apontando para a pasta do `libclang.dll` (LLVM, ou `pip install --user libclang`
  → pasta `clang\native`); CMake do VS Build Tools no `PATH`. Detalhes no README.
- `src-tauri/.cargo/config.toml` traz os flags do ggml (`/O2`, AVX2/FMA): **não remova**,
  sem eles o whisper fica ~10x mais lento.
- GPU é opt-in: `--features gpu-vulkan` ou `gpu-cuda`. O padrão (e o CI) é CPU.
- Dev: `npm run tauri dev`. Atenção: ele **reinicia o app a cada edição** em `src-tauri`.

## 5. Checks (rode antes de reportar)

- `npm run lint` (eslint + tsc) e `npm test` (vitest)
- `npm run lint:rust` (`cargo fmt --check` + `clippy --all-targets -D warnings`) e `npm run test:rust`
- `npm run check` roda tudo. Teste em cima de **dado/lógica pura**; rede só com servidor falso.

## 6. Slot do app (single-instance)

O Vyrp é **single-instance**: só uma cópia roda por vez, e quem testa ditado ponta a ponta
precisa dela sozinha. Regra:
1. Antes de abrir o app, **peça o slot ao Cérebro** (e confira que não há outro `vyrp.exe`).
2. Com o slot, abra, teste, **feche** e avise.
3. Se o Cérebro disser "fechado/liberado", obedeça na hora: feche o seu `vyrp.exe`
   e só reabra com o "liberado".
Nunca mate o `vyrp.exe` de outra pessoa sem avisar.

## 7. Evidência de teste

Teste de conteúdo = app aberto + print. Skill global **`test-evidence`**
(`evidencias.py criar <card> "<título>" ... --print`); config em `.evidencias.json`
(porta 8791, dados em `.evidencias/dados/`, ignorado pelo git). O reporte cita
`Evidência: http://localhost:8791/#<ID>` e diz o que o print mostra. Não deixe dado de
teste no AppData do usuário (modelos baixados, linhas no banco): limpe o que criou.

## 8. Git

- Branch de trabalho **`develop`**; **`main`** é release. Remoto `origin` (GitHub `peRodrigues/Vyrp`).
- **Conventional Commits em inglês, só o assunto** (`feat(hub): add ...`), sem corpo.
  Um commit por card; arquivo compartilhado vai no card mais tardio que o tocou.
- **Só o dono do git commita.** Hoje é o **Lima**. Os demais terminam, reportam e entregam
  a lista de arquivos; ninguém roda `commit/pull/merge/push` por conta própria.
- Nunca `reset --hard`, `push --force`, `stash`, nem resolver conflito escolhendo um lado às cegas.
- Menu depois de commitar: **1 commit / 2 commit + push / 3 nada** — espere a escolha do usuário.
- Antes de commitar, `lint:rust` e `test:rust` do estado que vai entrar (de preferência numa
  cópia limpa: `git worktree`).

## 9. Time do canvas

| Quem | Papel |
|---|---|
| **Cérebro** (terminal "Claude Code") | Maestro: planeja, delega, verifica e decide o slot do app |
| **Forja** | lead: hotkey, áudio, VAD, STT local, orquestrador (V11), Settings |
| **Torno** | lead: contexto do app (V12) e inserção (V15) |
| **Lima** | programador: telas do hub, módulos isolados (post, llm, storage, sound), docs; **dono do git** |
| **Git Manager** | git quando disponível (limite de uso): `sync`, `status`, commits agrupados |

Comunicação: `maestri list` antes de perguntar a alguém; `maestri ask "<nome>" "<msg>"`.
Todo reporte ao Cérebro termina com a linha **STATUS**:

```
[<card>] <título> / Mudou / Arquivos / Verificado / Evidência / Falta ou dúvidas /
STATUS: done | blocked: <motivo> | question: <pergunta> | waiting-user | escalate: <motivo>
```

Não espere perguntarem: o reporte é o sinal de que você está livre para o próximo card.
Mudança de contrato (§3) ou de direção (§1): `STATUS: escalate`.

## 10. Índice dos docs

- `README.md` — requisitos, build, GPU, como rodar.
- `docs/re/wispr-flow-spec.md` — spec clean-room: §5 arquitetura, §5.2 contratos, §6 cards (V2…V26).
- `.github/workflows/ci.yml` — o que o CI valida (Windows, CPU).
