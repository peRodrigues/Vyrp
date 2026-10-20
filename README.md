# Vyrp

Ditado por voz para Windows: segure um atalho, fale, solte, e o texto limpo
é colado no app em foco. Tauri 2 + Rust + React/TS. A STT é local (whisper.cpp),
com nuvem opcional; a limpeza por LLM é opcional e usa a chave do próprio usuário.

Arquitetura e plano de cards: [`docs/re/wispr-flow-spec.md`](docs/re/wispr-flow-spec.md)
(§5 arquitetura, §5.2 contratos, §6 cards).

## Requisitos

- Windows 10/11 com WebView2 (já vem no Windows 11)
- [Rust](https://rustup.rs) stable (toolchain MSVC) + Visual Studio Build Tools (C++)
- Node.js 24+

## Rodar

```powershell
npm install
npm run tauri dev        # app em modo dev (hot reload do front)
```

O app abre o **hub** (janela principal), o **overlay** (barra na borda
inferior) e um ícone no **tray**. Fechar o hub só esconde a janela; para
sair, use "Sair" no menu do tray. Os logs ficam em
`%LOCALAPPDATA%\app.vyrp.desktop\logs` (nível via `VYRP_LOG`, ex.: `debug`).

## Checagens

```powershell
npm run lint             # ESLint + TypeScript
npm test                 # Vitest
npm run lint:rust        # cargo fmt --check + clippy -D warnings
npm run test:rust        # cargo test
npm run check            # tudo acima
npm run tauri build -- --no-bundle   # exe release em src-tauri/target/release/vyrp.exe
npm run tauri build      # instaladores NSIS/MSI
```

O CI (`.github/workflows/ci.yml`, windows-latest) roda lint, testes e build.

## Estrutura

```
src/hub/          janela principal (React)
src/overlay/      barra de estado transparente (React)
src/shared/       tipos espelhados do Rust (eventos do ditado)
src-tauri/src/
  core/types.rs   contratos entre módulos (spec §5.2)
  hotkey/ audio/ vad/ stt/ context/ post/ llm/ insert/ storage/ settings/ metrics/
```

## Evidência de teste

O `.evidencias.json` configura a skill `test-evidence`: captura a janela
"Vyrp", com o painel em `http://localhost:8791/`. Os dados ficam em
`.evidencias/`, fora do git.
