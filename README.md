# aiprism

AI conversation pair collector for Claude Code and GitHub Copilot.

## Agent Support Status

| Agent | 대화 감지 | 파일 변경 감지 |
|-------|----------|--------------|
| Claude Code | ✅ 동작 | ✅ 동작 |
| GitHub Copilot | ✅ 동작 | ❌ 미지원 |
| Cursor | ❌ 미지원 | ❌ 미지원 |
| Gemini CLI | ❌ 미지원 | ❌ 미지원 |

## Overview
aiprism monitors AI coding assistant conversations and captures prompt/response pairs along with associated code changes.

## Features
- Detects user prompts and AI completions from log files
- Captures file diffs during the quiet period after each AI response
- Submits collected pairs to a remote server

## File Change Detection

`aiprism add /path/to/project` 로 등록한 프로젝트 경로 아래의 코드 파일 변경을 감지합니다.

### Watched File Extensions
`rs`, `py`, `ts`, `tsx`, `js`, `jsx`, `go`, `java`, `c`, `cpp`, `h`, `cs`, `rb`, `swift`, `kt`, `scala`, `php`, `html`, `css`, `scss`, `toml`, `yaml`, `yml`, `md`

### Excluded Directories
`.git`, `target`, `node_modules`, `.venv`, `__pycache__`, `dist`, `build`, `workflow`, `.claude`

> 감지 범위를 변경하려면 `src/watcher/file_watcher.rs`의 `CODE_EXTENSIONS`, `EXCLUDE_DIRS` 상수를 수정하세요.

## Usage

### Initialize
```bash
aiprism init --token YOUR_TOKEN
```
Creates `~/.aiprism/config.json` with default base URL `https://aiprism.dsj.co.kr/mcp/sse`.

Optional: specify custom base URL
```bash
aiprism init --token YOUR_TOKEN --base_url https://your-server.com
```

### Add project to watch
```bash
aiprism add /path/to/project
```
Adds the project to source_roots and generates MCP config files (`.mcp.json`, `.vscode/mcp.json`).

**Note**: Restart the daemon after adding new projects.

### Start daemon
```bash
aiprism
```
Monitors all registered projects for code changes and captures AI conversation pairs.
