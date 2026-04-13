# AI PRISM Local — Rust 재설계 설계서

## 1. 프로젝트 개요

### 목적
로컬 AI 코딩 도구(Claude Code, GitHub Copilot 등)와의 대화 내용과,
그로 인해 발생한 **실제 코드 변경 사항(Diff)** 을 pair로 묶어
AI PRISM 서버(Prompt Vault)의 collection에 자동 저장하는 고성능 백그라운드 엔진.

### 왜 Rust인가
기존 Python 구현의 구조적 한계:
- `rglob` 전체 파일 스캔 → GIL/GC로 인한 I/O 블로킹 → 파일 이벤트 유실
- `watchdog` 이벤트가 OS에 따라 중복 발생 → 동일 데이터 4개 저장
- Race Condition: 스냅샷 찍는 도중 AI가 이미 파일 수정 완료
- Collection 절단 기준 모호 (타이머 vs 로그 시그널 충돌)

Rust로 해결:
- OS 커널 수준 FS 이벤트 (`notify` crate) → 지연 없는 즉시 감지
- Zero-cost 비동기 (`tokio`) → 블로킹 없이 수백 개 이벤트 동시 처리
- Ownership 모델 → 세션 데이터 오염/중복 컴파일 단계에서 방지
- 단일 바이너리 배포 → pip, .egg-info 없음

---

## 2. 핵심 메커니즘

### 감지 대상 2가지

| 구분 | 대상 파일 | 역할 | 별칭 |
|------|----------|------|------|
| Log Watcher | Claude: `~/.claude/projects/**/*.jsonl`<br/>Copilot: `~/Library/Application Support/Code/User/workspaceStorage/*/chatSessions/*.jsonl` | Collection 시작/끝 결정, Pair 감지 | 지휘자 |
| File Watcher | 사용자 프로젝트 소스 코드 (`.rs`, `.py`, `.ts` 등) | 코드 변경 원본 확보 | 기록원 |

### Collection 경계 기준

**로그 파일 1개 = Collection 1개**

- Claude: `.jsonl` 파일 하나가 하나의 대화 세션 → 하나의 Collection
- Copilot: `chatSessions/*.jsonl` 파일 하나가 하나의 대화 세션 → 하나의 Collection
- CollectionManager는 **로그 파일 경로**를 키로 collection_id를 관리
- 같은 파일에서 감지된 Pair는 무조건 같은 Collection에 추가

### 확정적 2중 안전장치 (Deterministic Dual Trigger)

```
[장치 1 - Cut/Pair 경계 (PairManager 담당)]
로그에서 type:user 감지 → 새로운 user prompt
  → 현재 Pair가 처리 중인가? (Quiet Period 타이머 실행 중?)
    ├─ YES: 타이머 즉시 만료 → Pair 완료 신호 발송 후 새 Pair 시작
    └─ NO: 바로 새 Pair 시작
  (새로운 user prompt이 이전 Pair의 끝)

[장치 2 - Wait/Timer (PairManager 담당)]
로그에서 stop_reason:end_turn (Claude) 또는 isComplete (Copilot) 감지
  → 30~60초 Quiet Period 타이머 시작
  → 그 사이 파일 변경 시: 타이머 리셋
  → 정적이 지속되면: Pair 확정
  → PairManager → CollectionManager에 완성된 Pair 전달

[장치 3 - Collection 관리 (CollectionManager 담당)]
PairManager에서 완성된 Pair 수신 (로그 파일 경로 포함)
  → 해당 로그 파일 경로의 collection_id가 있나?
    ├─ YES: 기존 Collection에 Pair 추가
    └─ NO: 새 Collection 생성 후 Pair 추가, collection_id 저장
```

### Lazy Snapshot

- 모든 파일을 미리 스캔하지 않음
- File Watcher가 수정 이벤트를 받는 **그 즉시**, 해당 파일이 현재 Pair에서 처음 등장하면 원본을 메모리에 복사
- Pair 완료 시점에 `메모리 원본` vs `현재 파일` 비교 → Diff 생성
- Git 설치 여부 무관 (`similar` crate 사용)

### Log Watcher 시작 오프셋

- 프로그램 시작 시 기존 `.jsonl`을 처음부터 읽으면 과거 대화가 재전송됨
- `~/.aiprism/offsets.json`에 `파일경로 → 마지막 읽은 byte offset` 저장
- 시작 시 알려진 파일은 끝으로 skip, 새로 등장한 파일만 처음부터 읽음

### File Watcher 감시 제외 목록

아래 디렉토리는 감시 대상에서 제외:
- `.git/`, `target/`, `node_modules/`, `.venv/`, `__pycache__/`, `dist/`, `build/`

---

## 3. 전략 패턴 설계

### 3-1. LogParsingStrategy (에이전트 추상화)

```rust
#[async_trait]
pub trait LogParsingStrategy: Send + Sync {
    fn agent_name(&self) -> &str;
    fn target_extension(&self) -> &str;

    // 새 user prompt 감지 → 장치 1 트리거
    fn is_user_prompt(&self, line: &str) -> bool;

    // AI 완료 신호 감지 → 장치 2 트리거
    fn is_completion_signal(&self, line: &str) -> bool;

    // 로그 라인에서 request_id 추출
    // Claude: type:assistant의 uuid 필드
    // Copilot: requests[].id 필드
    fn extract_request_id(&self, line: &str) -> Option<String>;

    // Pair 구성에 필요한 메타데이터 추출
    // Claude: timestamp, model, cwd 등
    fn extract_metadata(&self, line: &str) -> Option<PairMetadata>;

    // user 메시지에서 텍스트 추출 (content 블록 배열 대응)
    // Claude: content[].type == "text" 인 것만 합산
    fn extract_user_text(&self, line: &str) -> Option<String>;

    // AI 응답에서 텍스트 추출
    fn extract_ai_text(&self, line: &str) -> Option<String>;
}

pub struct PairMetadata {
    pub timestamp: u64,
    pub model_id: String,
    pub context_file: String,  // cwd → project_path
    pub log_file_path: PathBuf, // Collection 키로 사용
}
```

구현체:
- `ClaudeStrategy`: `.jsonl`, `type:user` / `stop_reason:end_turn`
- `CopilotStrategy`: `.jsonl`, `requests[]` 배열 변화 / `isComplete` 플래그

### 3-2. PathStrategy (OS 추상화)

```rust
pub trait PathStrategy: Send + Sync {
    // 에이전트별 로그 감시 루트 경로 반환
    fn log_directories(&self) -> Vec<(String, PathBuf)>; // (agent_name, path)
    fn offset_store_path(&self) -> PathBuf;
    fn registry_store_path(&self) -> PathBuf;
}
```

구현체:
- `MacOSPathStrategy`:
  - Claude: `~/.claude/projects/`
  - Copilot: `~/Library/Application Support/Code/User/workspaceStorage/`
- `WindowsPathStrategy`: `%APPDATA%/Claude/projects`, `%APPDATA%/Code/User/...`
- `LinuxPathStrategy`: `~/.claude/projects`, `~/.config/Code/User/...`

런타임 또는 컴파일 시점(`cfg(target_os)`)에 주입.

---

## 4. 프로젝트 구조

```
aiprism/
├── Cargo.toml
└── src/
    ├── main.rs                    # CLI 진입점, Tokio 런타임 초기화, 전략 주입
    ├── config.rs                  # ~/.aiprism/config.json 로드, API 토큰 관리
    │
    ├── strategy/                  # [전략 패턴 핵심]
    │   ├── mod.rs                 # LogParsingStrategy, PathStrategy Trait 정의
    │   ├── log/
    │   │   ├── mod.rs
    │   │   ├── claude.rs          # Claude Code .jsonl 파싱 (type:user, end_turn)
    │   │   └── copilot.rs         # GitHub Copilot .jsonl 파싱 (requests[], isComplete)
    │   └── path/
    │       ├── mod.rs
    │       ├── macos.rs           # ~/Library/... 경로
    │       ├── windows.rs         # %APPDATA%/... 경로
    │       └── linux.rs           # ~/.config/... 경로
    │
    ├── watcher/                   # [실시간 감시 엔진]
    │   ├── mod.rs                 # 두 Watcher 통합 관리
    │   ├── log_watcher.rs         # 에이전트 로그 감시 (지휘자)
    │   │                          #   linemux로 tail, LogParsingStrategy 호출
    │   │                          #   Pair 감지 및 PairManager에 신호 전달
    │   └── file_watcher.rs        # 소스 코드 감시 (기록원)
    │                              #   notify로 즉시 감지, Lazy Snapshot 실행
    │                              #   제외 디렉토리 필터링
    │
    ├── pair/                      # [Pair 관리 (PairManager)]
    │   ├── mod.rs                 # PairManager: Pair 생명주기 관리
    │   │                          #   Arc<Mutex<ActivePair>> 상태 관리
    │   │                          #   Quiet Period 타이머, Lazy Snapshot, Diff 생성
    │   └── diff.rs                # similar crate 기반 Diff 생성, Markdown 포맷
    │
    ├── collection/                # [Collection 관리 (CollectionManager)]
    │   └── mod.rs                 # CollectionManager: Collection 생성/추가
    │                              #   로그 파일 경로 → collection_id 매핑
    │                              #   registry.json 영속화
    │
    ├── models/                    # [데이터 모델]
    │   └── mod.rs                 # Pair, ActivePair, CollectionState, PairPayload
    │
    └── network/                   # [서버 통신]
        ├── mod.rs
        └── sender.rs              # reqwest 기반 API 전송, 재시도
```

---

## 5. 데이터 흐름

```
[사용자가 AI에게 질문]
        ↓
[Log Watcher] type:user 감지 → Pair 시작
        ↓
[LogParsingStrategy] user_text + request_id + metadata(log_file_path 포함) 추출
        ↓
[PairManager] Pair 경계 판단
        ├─ 현재 Pair 처리 중? → Quiet Period 타이머 즉시 만료 대기
        └─ 완료 또는 없음 → 새 ActivePair 시작 (request_id로 중복 체크)
        ↓
[AI가 코드 파일 수정]
        ↓
[File Watcher] Modified 이벤트 즉시 감지 (제외 디렉토리 필터 후)
        ↓
[Lazy Snapshot] 현재 Pair에서 처음 등장한 파일 → 원본을 메모리 복사
        ↓
[Log Watcher] end_turn / isComplete 감지 + ai_text 추출
        ↓
[PairManager] Quiet Period 타이머 시작 (30~60초)
        ↓  (파일 변경 시 타이머 리셋)
[타이머 만료]
        ↓
[Diff 생성] 메모리 원본 vs 현재 파일 → 파일별 코드블록 Markdown
        ↓
[완성된 Pair 구성] user_query + ai_response + code_changes + log_file_path
        ↓
[CollectionManager] log_file_path 기준으로 collection_id 조회
        ├─ YES: 기존 Collection에 Pair 추가
        └─ NO: 새 Collection 생성 → collection_id 저장 → Pair 추가
        ↓
[Sender] PairPayload → POST /api/prompt-groups/mcp-save
```

---

## 6. 핵심 데이터 구조

```rust
// models/mod.rs

pub struct Pair {
    pub source: String,         // "claude-code" | "copilot"
    pub request_id: String,     // 에이전트 로그에서 추출한 UUID (중복 방지)
                                // Claude: type:assistant의 uuid 필드
                                // Copilot: requests[].id 필드
    pub timestamp: u64,
    pub model_id: String,
    pub user_query: String,
    pub ai_response: String,
    pub context_file: String,   // cwd → project_path로 서버 전송
    pub code_changes: String,   // 파일별 Diff (Markdown 코드블록)
    pub log_file_path: PathBuf, // 소속 Collection 결정 키
}

pub struct ActivePair {
    pub request_id: String,     // 로그 파싱 시 추출한 UUID
    pub user_query: String,     // type:user 라인에서 추출
    pub ai_response: String,    // type:assistant end_turn 라인에서 추출
    pub context_file: String,
    pub log_file_path: PathBuf,
    // path → 수정 전 원본 (Lazy Snapshot)
    pub snapshots: HashMap<PathBuf, String>,
    // 현재 Pair에서 변경된 파일 목록
    pub dirty_files: HashSet<PathBuf>,
    pub last_activity: Instant,
    pub quiet_timer_active: bool,
}

pub struct CollectionState {
    // 키: log_file_path
    pub collection_id: Option<u64>, // None = 신규, Some(id) = 기존
    pub source: String,             // "claude-code" | "copilot"
}

// 서버 API 요청 데이터 구조
pub struct PairPayload {
    pub user_query: String,
    pub ai_response: String,    // ai_response + "\n\n## Code Changes\n" + code_changes
    pub project_path: String,
    pub collection_id: Option<u64>,
    // 신규 Collection만 포함
    pub tags: Option<Vec<String>>,  // e.g., ["claudecode"] 또는 ["GitHub Copilot"]
    pub title: Option<String>,      // e.g., "claudecode 2026-04-05T14:32:00+09:00"
}

// tag/title 생성 규칙 (신규 Collection 시)
// agent    → tag              → title 예시
// Claude   → "claudecode"     → "claudecode 2026-04-05T14:32:00+09:00"
// Copilot  → "GitHub Copilot" → "GitHub Copilot 2026-04-05T14:32:00+09:00"
// title = format!("{} {}", tag, chrono::Local::now().to_rfc3339())
```

---

## 7. 기술 스택

| 역할 | Crate |
|------|-------|
| 비동기 런타임 | `tokio` |
| FS 이벤트 감지 | `notify` |
| 로그 파일 tail | `linemux` |
| Diff 생성 | `similar` |
| HTTP 클라이언트 | `reqwest` |
| CLI | `clap` |
| 직렬화 | `serde`, `serde_json` |
| OS 경로 | `dirs` |
| 날짜/시간 | `chrono` |

---

## 8. Pair → 서버 전송 흐름

```
[완성된 Pair]
├─ user_query: "사용자 질문"
├─ ai_response: "AI 답변"
├─ context_file: "/Users/xxx/project"
├─ code_changes: "### path/to/file.rs\n\n```diff\n...\n```"
├─ request_id: "uuid"
└─ log_file_path: "~/.claude/projects/abc123.jsonl"

    ↓

[ai_response 최종 구성]
"AI 답변\n\n## Code Changes\n\n### path/to/file.rs\n\n```diff\n...\n```"

    ↓

[PairPayload 생성]
{
  "user_query": "사용자 질문",
  "ai_response": "위의 최종 구성",
  "project_path": "/Users/xxx/project",
  "collection_id": 1239 (또는 null),
  // 신규 Collection일 때만 포함:
  "tags": ["claudecode"],                          // Claude의 경우
  "title": "claudecode 2026-04-05T14:32:00+09:00" // tag + 생성 시각 (RFC3339)
}

    ↓

[Sender]
→ collection_id 확인
  ├─ null: POST /api/prompt-groups/mcp-save (신규 Collection 생성)
  │        응답: {"id": 1239}
  │        CollectionManager → registry.json에 log_file_path → 1239 저장
  └─ int:  POST /api/prompt-groups/mcp-save (기존 Collection에 Pair 추가)
```

---

## 9. 영속화 파일

| 파일 | 위치 | 내용 |
|------|------|------|
| `offsets.json` | `~/.aiprism/offsets.json` | 로그 파일 경로 → 마지막 읽은 byte offset |
| `registry.json` | `~/.aiprism/registry.json` | 로그 파일 경로 → collection_id |

---

## 10. 향후 확장 포인트

- `CursorStrategy` 추가: `LogParsingStrategy` 구현체만 신규 작성
- 새 OS 지원: `PathStrategy` 구현체만 신규 작성
- 전송 실패 시 로컬 큐(오프라인 버퍼): `sender.rs`에 재시도 로직 추가
- 다중 프로젝트 동시 감시: `config.rs`에서 복수 `source_root` 지원
