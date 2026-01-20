# AI Orchestrator UI

A lightweight, cost-optimized orchestrator for managing multiple Claude Code instances from a unified web interface.

## Project Overview

This application provides a clean HTMX-based UI that allows authenticated users to connect to and manage all their Claude Code instances running anywhere. It aggregates instances of [claudecodeui](https://github.com/siteboon/claudecodeui) which are modified to connect back to this orchestrator.

### Core Requirements

| Requirement        | Implementation                                        |
| ------------------ | ----------------------------------------------------- |
| UI Framework       | HTMX for hypermedia-driven interactions               |
| Backend            | Rust compiled to WebAssembly for Cloudflare Workers   |
| Client Aggregation | Modified claudecodeui instances connect via WebSocket |
| Authentication     | GitHub App with org/user/team restrictions            |
| Platform           | Cloudflare Workers, D1, R2                            |
| Cost Optimization  | Durable Objects, minimal CPU/memory, sleep when idle  |

## Architecture

```
┌─────────────────┐     ┌──────────────────────────────────────────────┐
│  claudecodeui   │────▶│           Cloudflare Edge                    │
│   instance 1    │     │  ┌─────────────────────────────────────────┐ │
└─────────────────┘     │  │         Rust/WASM Worker                │ │
                        │  │  ┌─────────────┐  ┌──────────────────┐  │ │
┌─────────────────┐     │  │  │ HTMX Routes │  │ GitHub OAuth     │  │ │
│  claudecodeui   │────▶│  │  └─────────────┘  └──────────────────┘  │ │
│   instance 2    │     │  └─────────────────────────────────────────┘ │
└─────────────────┘     │                       │                      │
                        │  ┌────────────────────▼────────────────────┐ │
┌─────────────────┐     │  │        Durable Object (per user)       │ │
│  claudecodeui   │────▶│  │  ┌───────────┐  ┌───────────────────┐  │ │
│   instance N    │     │  │  │ WebSocket │  │ Client Registry   │  │ │
└─────────────────┘     │  │  │   Hub     │  │ (SQLite storage)  │  │ │
                        │  │  └───────────┘  └───────────────────┘  │ │
     ┌──────────┐       │  └─────────────────────────────────────────┘ │
     │ Browser  │◀─────▶│                       │                      │
     │ (HTMX)   │       │  ┌────────────────────▼────────────────────┐ │
     └──────────┘       │  │  D1 (users, sessions)  R2 (static assets)│ │
                        │  └─────────────────────────────────────────┘ │
                        └──────────────────────────────────────────────┘
```

## Technology Stack

### Backend (Rust on Cloudflare Workers)

- **worker-rs**: Rust bindings for Cloudflare Workers
- **worker-macros**: Procedural macros for route handling
- **serde**: JSON serialization
- **wasm-bindgen**: WebAssembly interop

### Frontend

- **HTMX**: Hypermedia-driven UI updates
- **Minimal CSS**: Lightweight styling (no heavy frameworks)
- **No build step**: Direct HTML templates

### Cloudflare Services

| Service         | Purpose                                                 |
| --------------- | ------------------------------------------------------- |
| Workers         | HTTP routing, HTMX responses, OAuth flow                |
| Durable Objects | Per-user WebSocket hub, client registry, real-time sync |
| D1              | User accounts, sessions, GitHub team/org mappings       |
| R2              | Static assets (CSS, JS), optional log storage           |
| KV              | Session tokens, rate limiting (optional)                |

## Development Guidelines

### Rust/WASM Best Practices

```rust
// Use worker-rs for Cloudflare Workers
use worker::*;

#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    Router::new()
        .get("/", |_, _| Response::ok("Hello"))
        .get_async("/clients", handle_clients)
        .run(req, env)
        .await
}
```

### HTMX Response Patterns

Return HTML fragments, not JSON:

```rust
// Good: Return HTML fragment for HTMX swap
async fn get_client_list(env: &Env, user_id: &str) -> Result<Response> {
    let clients = fetch_clients(env, user_id).await?;
    let html = render_client_list(&clients);
    Response::from_html(html)
}

// HTMX attributes in templates
// hx-get="/clients" hx-trigger="every 5s" hx-swap="innerHTML"
```

### Durable Object Pattern (Per-User Hub)

```rust
// One DO per authenticated user - handles all their connected clients
#[durable_object]
pub struct UserHub {
    state: State,
    env: Env,
    clients: HashMap<String, ClientConnection>,
}

impl UserHub {
    // WebSocket connections from claudecodeui instances
    async fn websocket_message(&mut self, ws: WebSocket, msg: String) {
        // Route messages, update client state, broadcast to browser
    }
}
```

### Cost Optimization Rules

1. **Sleep when idle**: Durable Objects hibernate after 10 seconds of inactivity
2. **Batch D1 queries**: Combine reads/writes where possible
3. **Minimize CPU time**: Keep request handlers under 50ms
4. **Use SQLite in DOs**: Prefer DO's built-in SQLite over D1 for hot data
5. **Lazy load clients**: Only fetch client details when user expands a card
6. **Cache static assets**: R2 with long cache headers for CSS/JS

### GitHub App Authentication

```rust
// OAuth flow endpoints
Router::new()
    .get("/auth/github", start_oauth)           // Redirect to GitHub
    .get("/auth/github/callback", handle_callback)  // Exchange code for token
    .get("/auth/logout", logout)
```

Required GitHub App permissions:

- `read:org` - Check organization membership
- `read:user` - Get user profile
- `user:email` - Get email for account linking

Team/org restriction logic:

```rust
async fn authorize_user(token: &str, allowed_orgs: &[String]) -> Result<bool> {
    let orgs = github_api::get_user_orgs(token).await?;
    Ok(orgs.iter().any(|o| allowed_orgs.contains(&o.login)))
}
```

### Client Connection Protocol

claudecodeui instances connect with:

```javascript
// Modified claudecodeui connection
const ws = new WebSocket("wss://orchestrator.example.com/ws/connect");
ws.send(
  JSON.stringify({
    type: "register",
    client_id: "unique-client-id",
    user_token: "user-auth-token",
    metadata: {
      hostname: "dev-machine",
      project: "/path/to/project",
      status: "idle",
    },
  }),
);
```

### File Structure (Implemented)

```
ai-orchestrator/
├── Cargo.toml                      # Rust dependencies (worker-rs 0.7)
├── wrangler.toml                   # Cloudflare Workers configuration
├── schema.sql                      # D1 database schema
├── .gitignore                      # Build artifacts, secrets
└── src/
    ├── lib.rs                      # Worker entry point with routes
    │                               # Routes: /, /health, /auth/*, /dashboard,
    │                               # /clients, /clients/:id, /ws/connect, /static/*
    ├── auth/
    │   ├── mod.rs                  # GitHub OAuth flow (start, callback, logout)
    │   │                           # - CSRF state validation
    │   │                           # - Token exchange with GitHub API
    │   │                           # - Org/user access restrictions
    │   │                           # - Base64-encoded session cookies
    │   └── middleware.rs           # Session validation from cookies
    │                               # - Decodes session data
    │                               # - Checks expiration
    │                               # - Returns User or redirect
    ├── handlers/
    │   ├── mod.rs                  # Home page, health check, static assets (R2)
    │   ├── clients.rs              # HTMX endpoints for client list/cards
    │   │                           # - GET /clients → client list partial
    │   │                           # - GET /clients/:id → single card partial
    │   ├── dashboard.rs            # Main dashboard page (requires auth)
    │   └── websocket.rs            # WebSocket upgrade → routes to UserHub DO
    │                               # - Browser connections (type=browser)
    │                               # - claudecodeui connections (token=xxx)
    ├── durable_objects/
    │   ├── mod.rs                  # Exports UserHub
    │   └── user_hub.rs             # Per-user Durable Object
    │                               # - WebSocket hub for all user's clients
    │                               # - Client registry with RefCell<HashMap>
    │                               # - Browser list for real-time broadcasts
    │                               # - Message types: Register, StatusUpdate,
    │                               #   Ping/Pong, GetClients, ClientUpdate
    ├── models/
    │   ├── mod.rs                  # Exports User, Session, Client types
    │   ├── user.rs                 # User and Session models
    │   │                           # - ID generation (getrandom)
    │   │                           # - Timestamp handling (js_sys::Date)
    │   └── client.rs               # Client and ClientMetadata models
    │                               # - ClientStatus: Idle, Active, Busy, Disconnected
    └── templates/
        └── mod.rs                  # Inline HTMX templates (no separate files)
                                    # - render_home() → login page
                                    # - render_dashboard() → main UI with WebSocket
                                    # - render_client_list() → cards grid partial
                                    # - render_client_card() → single client card
                                    # - Dark theme CSS included inline
```

### wrangler.jsonc Configuration

```jsonc
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "ai-orchestrator",
  "main": "build/worker/shim.mjs",
  "compatibility_date": "2026-01-01",

  "build": {
    "command": "cargo install -q worker-build && worker-build --release",
  },

  "durable_objects": {
    "bindings": [{ "name": "USER_HUB", "class_name": "UserHub" }],
  },

  "migrations": [{ "tag": "v1", "new_sqlite_classes": ["UserHub"] }],

  "d1_databases": [
    {
      "binding": "DB",
      "database_name": "orchestrator-db",
      "database_id": "<ID>",
    },
  ],

  "r2_buckets": [{ "binding": "ASSETS", "bucket_name": "orchestrator-assets" }],

  "vars": {
    "GITHUB_CLIENT_ID": "",
    "ALLOWED_ORGS": "",
    "ALLOWED_USERS": "",
    "ALLOWED_TEAMS": "",
  },
}
```

### D1 Schema

```sql
-- users table
CREATE TABLE users (
    id TEXT PRIMARY KEY,
    github_id INTEGER UNIQUE NOT NULL,
    github_login TEXT NOT NULL,
    email TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    last_login DATETIME
);

-- sessions table
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    expires_at DATETIME NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- allowed_entities (orgs, users, teams)
CREATE TABLE allowed_entities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL CHECK (entity_type IN ('org', 'user', 'team')),
    entity_id TEXT NOT NULL,
    entity_name TEXT NOT NULL,
    UNIQUE(entity_type, entity_id)
);

CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expires ON sessions(expires_at);
CREATE INDEX idx_allowed_type ON allowed_entities(entity_type);
```

## Anti-Patterns (NEVER)

- **No heavy frameworks**: Don't use React, Vue, or large CSS frameworks
- **No long-running requests**: Keep all handlers under 30 seconds (Workers limit)
- **No global state in Worker**: All state belongs in Durable Objects or D1
- **No polling from browser**: Use WebSocket for real-time updates, HTMX triggers for periodic checks
- **No secrets in code**: Use wrangler secrets for `GITHUB_CLIENT_SECRET`
- **No unnecessary wakeups**: Don't ping DOs to keep them alive

## Testing

```bash
# Local development
wrangler dev

# Run Rust tests
cargo test

# Deploy to staging
wrangler deploy --env staging

# Deploy to production
wrangler deploy
```

## Security Considerations

1. **Validate all WebSocket messages**: Never trust client-provided data
2. **Rate limit connections**: Use KV to track connection attempts per IP
3. **Rotate session tokens**: Short-lived sessions with refresh tokens
4. **Verify GitHub tokens**: Re-validate org/team membership periodically
5. **Sanitize HTML output**: Escape all user-provided data in templates

## Performance Targets

| Metric                 | Target             |
| ---------------------- | ------------------ |
| First contentful paint | < 500ms            |
| HTMX partial update    | < 100ms            |
| WebSocket latency      | < 50ms             |
| Worker CPU time        | < 50ms per request |
| DO hibernation         | Within 10s of idle |

## Deployment Checklist

- [ ] Set `GITHUB_CLIENT_SECRET` via `wrangler secret put`
- [ ] Configure allowed orgs/users/teams in D1
- [ ] Create D1 database and run migrations
- [ ] Create R2 bucket for static assets
- [ ] Set up custom domain with SSL
- [ ] Configure GitHub App callback URL
- [ ] Test OAuth flow end-to-end
- [ ] Verify WebSocket connections from claudecodeui

---

## Work Plan

### Phase 1: Core Infrastructure (Current - Scaffold Complete)

| Task                                | Status  | Notes                           |
| ----------------------------------- | ------- | ------------------------------- |
| Project scaffold with worker-rs 0.7 | ✅ Done | Compiles successfully           |
| Router with all route stubs         | ✅ Done | See `src/lib.rs`                |
| GitHub OAuth flow                   | ✅ Done | Cookie-based sessions           |
| Auth middleware                     | ✅ Done | Session validation              |
| UserHub Durable Object              | ✅ Done | RefCell for interior mutability |
| HTMX templates (inline)             | ✅ Done | Dark theme dashboard            |
| D1 schema                           | ✅ Done | `schema.sql`                    |
| wrangler.toml config                | ✅ Done | DO, D1, R2 bindings             |

### Phase 2: Orchestrator Backend Completion

| Task                                | Priority | Description                                                                                                      |
| ----------------------------------- | -------- | ---------------------------------------------------------------------------------------------------------------- |
| **WebSocket hibernation handlers**  | P0       | Implement `websocket_message`, `websocket_close`, `websocket_error` trait methods in UserHub for hibernation API |
| **D1 integration for sessions**     | P1       | Replace cookie-based sessions with D1 storage for security and revocation                                        |
| **Client persistence in DO SQLite** | P1       | Store client registry in DO's built-in SQLite for crash recovery                                                 |
| **Token generation for clients**    | P1       | Generate secure tokens for claudecodeui to authenticate with                                                     |
| **API for token management**        | P1       | `/api/tokens` - create, list, revoke client connection tokens                                                    |
| **Rate limiting**                   | P2       | Use KV to track WebSocket connection attempts per IP                                                             |
| **Health check with metrics**       | P2       | Return connected client count, DO status                                                                         |
| **Error handling improvements**     | P2       | Consistent error responses, logging                                                                              |

### Phase 3: Dashboard UI Enhancements

| Task                         | Priority | Description                                                              |
| ---------------------------- | -------- | ------------------------------------------------------------------------ |
| **Client detail view**       | P1       | Expandable card showing full project path, connection time, activity log |
| **Client actions**           | P1       | Disconnect client, view logs, send commands                              |
| **Real-time status updates** | P1       | WebSocket → HTMX swap for live status changes                            |
| **Token management UI**      | P1       | Generate/revoke tokens from dashboard                                    |
| **Notification badges**      | P2       | Show count of active/busy clients                                        |
| **Mobile responsive layout** | P2       | CSS adjustments for small screens                                        |
| **Keyboard shortcuts**       | P3       | Navigate between clients, quick actions                                  |

### Phase 4: claudecodeui Modifications

claudecodeui needs modifications to connect back to the orchestrator. These changes should be minimal and optional (fallback to standalone mode).

#### 4.1 Configuration

```javascript
// New config options in claudecodeui
{
  "orchestrator": {
    "enabled": true,
    "url": "wss://orchestrator.example.com/ws/connect",
    "token": "user-generated-token",
    "reconnect_interval": 5000,
    "heartbeat_interval": 30000
  }
}
```

#### 4.2 Connection Manager

| Task                         | File                  | Description                                  |
| ---------------------------- | --------------------- | -------------------------------------------- |
| **OrchestratorClient class** | `src/orchestrator.ts` | WebSocket client with auto-reconnect         |
| **Status reporter**          | `src/orchestrator.ts` | Report idle/active/busy status changes       |
| **Heartbeat handler**        | `src/orchestrator.ts` | Send ping, handle pong, detect disconnection |
| **Graceful degradation**     | `src/orchestrator.ts` | Continue working if orchestrator unavailable |

#### 4.3 Message Protocol (claudecodeui → Orchestrator)

```typescript
// Messages sent by claudecodeui
interface RegisterMessage {
  type: "register";
  client_id: string; // Unique per instance (hostname + process ID)
  user_token: string; // Token from orchestrator dashboard
  metadata: {
    hostname: string;
    project: string; // Current working directory
    status: "idle" | "active" | "busy";
    version: string; // claudecodeui version
  };
}

interface StatusUpdateMessage {
  type: "status_update";
  client_id: string;
  status: "idle" | "active" | "busy";
}

interface PingMessage {
  type: "ping";
  client_id: string;
}
```

#### 4.4 Message Protocol (Orchestrator → claudecodeui)

```typescript
// Messages received by claudecodeui
interface PongMessage {
  type: "pong";
  client_id: string;
}

interface CommandMessage {
  type: "command";
  command: "disconnect" | "refresh_status";
}

interface ErrorMessage {
  type: "error";
  message: string;
}
```

#### 4.5 Implementation Tasks

| Task                               | Priority | Description                                |
| ---------------------------------- | -------- | ------------------------------------------ |
| **Add orchestrator config schema** | P0       | JSON schema for config validation          |
| **Create OrchestratorClient**      | P0       | WebSocket connection with reconnect logic  |
| **Hook into status changes**       | P0       | Detect when Claude is thinking/responding  |
| **Add CLI flag for token**         | P1       | `--orchestrator-token` for headless setups |
| **Environment variable support**   | P1       | `ORCHESTRATOR_URL`, `ORCHESTRATOR_TOKEN`   |
| **Connection status indicator**    | P2       | Show connected/disconnected in UI          |
| **Handle disconnect command**      | P2       | Clean shutdown when orchestrator requests  |

### Phase 5: Deployment & Operations

| Task                           | Priority | Description                            |
| ------------------------------ | -------- | -------------------------------------- |
| **Create GitHub App**          | P0       | OAuth credentials, callback URLs       |
| **Deploy to Cloudflare**       | P0       | Initial deployment with wrangler       |
| **Set up custom domain**       | P1       | DNS, SSL certificate                   |
| **Create staging environment** | P1       | Separate D1, different allowed users   |
| **Monitoring & alerts**        | P2       | Cloudflare analytics, error tracking   |
| **Backup strategy for D1**     | P2       | Regular exports of user/session data   |
| **Documentation**              | P2       | Setup guide, API docs, troubleshooting |

### Phase 6: Future Enhancements

| Feature                   | Description                                            |
| ------------------------- | ------------------------------------------------------ |
| **Multi-user workspaces** | Share client visibility with team members              |
| **Command relay**         | Send commands to claudecodeui instances from dashboard |
| **Activity timeline**     | Historical view of client activity                     |
| **Webhooks**              | Notify external systems of status changes              |
| **Mobile app**            | Native iOS/Android for quick status checks             |
| **CLI tool**              | `orchestrator-cli` for scripted management             |

---

## Next Immediate Steps

1. **Test local development**: Run `wrangler dev` and verify routes work
2. **Create GitHub App**: Get OAuth credentials for testing
3. **Deploy scaffold**: Initial deployment to get real URL for callback
4. **Implement WebSocket handlers**: Complete UserHub hibernation API
5. **Fork claudecodeui**: Start on orchestrator client implementation
