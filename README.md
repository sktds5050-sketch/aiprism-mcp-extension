# aiprism

AI conversation pair collector for Claude Code and GitHub Copilot.

## Overview
aiprism monitors AI coding assistant conversations and captures prompt/response pairs along with associated code changes.

## Features
- Detects user prompts and AI completions from log files
- Captures file diffs during the quiet period after each AI response
- Submits collected pairs to a remote server

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
