use futures::channel::oneshot;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::JsValue;
use worker::{SqlStorageValue, *};

use crate::models::{Client, ClientMetadata, ClientStatus};

/// Row structure for deserializing SQLite client rows
#[derive(Debug, Deserialize)]
struct ClientRow {
    client_id: String,
    user_id: String,
    hostname: String,
    project: String,
    status: String,
    last_activity: Option<String>,
    connected_at: String,
    last_seen: String,
    callback_url: Option<String>,
}

/// HTTP proxy request from the Worker
#[derive(Debug, Serialize, Deserialize)]
pub struct ProxyRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub query: Option<String>,
}

/// HTTP proxy response to the Worker
#[derive(Debug, Serialize, Deserialize)]
pub struct ProxyResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Message types for WebSocket communication
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    /// Client registration from claudecodeui
    Register {
        client_id: String,
        user_token: String,
        metadata: ClientMetadata,
    },
    /// Registration response
    Registered {
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// Status update from client
    StatusUpdate {
        client_id: String,
        status: ClientStatus,
    },
    /// Heartbeat/ping
    Ping { client_id: String },
    /// Pong response
    Pong { client_id: String },
    /// Client list request (from browser)
    GetClients,
    /// Client list response
    ClientList { clients: Vec<Client> },
    /// Single client update broadcast
    ClientUpdate { client: Client },
    /// Client disconnected
    ClientDisconnected { client_id: String },
    /// Error message
    Error { message: String },
    /// Connect to client request (from browser)
    ConnectClient { client_id: String },
    /// Connect response (to browser)
    ConnectResponse {
        success: bool,
        client_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    // ============ Message Forwarding (Browser <-> claudecodeui) ============
    /// Forward a request to a claudecodeui client (from browser)
    ForwardToClient {
        client_id: String,
        request_id: String,
        action: String,
        #[serde(default)]
        payload: serde_json::Value,
    },
    /// User request forwarded to claudecodeui (orchestrator -> claudecodeui)
    UserRequest {
        request_id: String,
        action: String,
        #[serde(default)]
        payload: serde_json::Value,
    },
    /// Response chunk from claudecodeui (claudecodeui -> orchestrator)
    ResponseChunk {
        request_id: String,
        data: serde_json::Value,
    },
    /// Response complete from claudecodeui (claudecodeui -> orchestrator)
    ResponseComplete {
        request_id: String,
        #[serde(default)]
        data: Option<serde_json::Value>,
    },
    /// Forwarded response to browser (orchestrator -> browser)
    ForwardedResponse {
        client_id: String,
        request_id: String,
        data: serde_json::Value,
        complete: bool,
    },
    // ============ HTTP Proxy Messages (via WebSocket) ============
    /// HTTP proxy request (orchestrator -> claudecodeui)
    HttpProxyRequest {
        request_id: String,
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        /// Base path for URL rewriting (e.g., "/clients/{client_id}/proxy")
        #[serde(skip_serializing_if = "Option::is_none")]
        proxy_base: Option<String>,
    },
    /// HTTP proxy response (claudecodeui -> orchestrator)
    HttpProxyResponse {
        request_id: String,
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
}

struct ClientConnection {
    websocket: WebSocket,
    client: Client,
}

/// Tracks a pending forwarded request
struct PendingRequest {
    client_id: String,
    browser_ws: WebSocket,
}

/// Per-user Durable Object that manages connected claudecodeui instances
#[durable_object]
pub struct UserHub {
    state: State,
    env: Env,
    /// Connected claudecodeui clients (using RefCell for interior mutability)
    clients: RefCell<HashMap<String, ClientConnection>>,
    /// Connected browser sessions (for real-time updates)
    browsers: RefCell<Vec<WebSocket>>,
    /// Whether SQLite storage has been initialized
    initialized: RefCell<bool>,
    /// Pending requests: request_id -> (client_id, browser_ws)
    pending_requests: RefCell<HashMap<String, PendingRequest>>,
    /// Pending HTTP proxy requests: request_id -> oneshot sender for response
    pending_proxy_requests: RefCell<HashMap<String, oneshot::Sender<ProxyResponse>>>,
}

impl DurableObject for UserHub {
    fn new(state: State, env: Env) -> Self {
        Self {
            state,
            env,
            clients: RefCell::new(HashMap::new()),
            browsers: RefCell::new(Vec::new()),
            initialized: RefCell::new(false),
            pending_proxy_requests: RefCell::new(HashMap::new()),
            pending_requests: RefCell::new(HashMap::new()),
        }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let url = req.url()?;
        let path = url.path();

        // Route based on path
        if path == "/ws" {
            self.handle_websocket(req).await
        } else if path == "/clients" {
            self.get_clients_json()
        } else if path.starts_with("/clients/") && path.ends_with("/disconnect") {
            // Extract client_id from /clients/{id}/disconnect
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() >= 3 {
                let client_id = parts[2];
                self.disconnect_client(client_id).await
            } else {
                Response::error("Invalid path", 400)
            }
        } else if path.starts_with("/proxy/") {
            // Extract client_id from /proxy/{client_id}
            let client_id = path.strip_prefix("/proxy/").unwrap_or("");
            if client_id.is_empty() {
                Response::error("Missing client ID", 400)
            } else {
                self.handle_proxy(req, client_id).await
            }
        } else {
            Response::error("Not found", 404)
        }
    }

    /// Handle incoming WebSocket messages (hibernation API)
    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        match message {
            WebSocketIncomingMessage::String(text) => {
                self.handle_message(&ws, &text).await?;
            }
            WebSocketIncomingMessage::Binary(_) => {
                // Binary messages not supported
                let error = WsMessage::Error {
                    message: "Binary messages not supported".to_string(),
                };
                if let Ok(json) = serde_json::to_string(&error) {
                    let _ = ws.send_with_str(&json);
                }
            }
        }
        Ok(())
    }

    /// Handle WebSocket close events (hibernation API)
    async fn websocket_close(
        &self,
        ws: WebSocket,
        _code: usize,
        _reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        self.handle_close(&ws).await;
        Ok(())
    }

    /// Handle WebSocket errors (hibernation API)
    async fn websocket_error(&self, ws: WebSocket, _error: Error) -> Result<()> {
        // Treat errors as disconnections
        self.handle_close(&ws).await;
        Ok(())
    }
}

impl UserHub {
    /// Initialize SQLite schema if not already done
    fn ensure_initialized(&self) -> Result<()> {
        if *self.initialized.borrow() {
            return Ok(());
        }

        let sql = self.state.storage().sql();
        sql.exec(
            "CREATE TABLE IF NOT EXISTS clients (
                client_id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                hostname TEXT NOT NULL,
                project TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'idle',
                last_activity TEXT,
                connected_at TEXT NOT NULL,
                last_seen TEXT NOT NULL,
                callback_url TEXT
            )",
            None,
        )?;

        // Migration: Add callback_url column if it doesn't exist (ignore error if already exists)
        let _ = sql.exec(
            "ALTER TABLE clients ADD COLUMN callback_url TEXT",
            None,
        );

        *self.initialized.borrow_mut() = true;
        Ok(())
    }

    /// Save client to SQLite
    fn save_client(&self, client: &Client) -> Result<()> {
        self.ensure_initialized()?;
        let sql = self.state.storage().sql();

        sql.exec(
            "INSERT OR REPLACE INTO clients (client_id, user_id, hostname, project, status, last_activity, connected_at, last_seen, callback_url)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            Some(vec![
                SqlStorageValue::String(client.id.clone()),
                SqlStorageValue::String(client.user_id.clone()),
                SqlStorageValue::String(client.metadata.hostname.clone()),
                SqlStorageValue::String(client.metadata.project.clone()),
                SqlStorageValue::String(client.metadata.status.to_string()),
                client.metadata.last_activity.clone().map(SqlStorageValue::String).unwrap_or(SqlStorageValue::Null),
                SqlStorageValue::String(client.connected_at.clone()),
                SqlStorageValue::String(client.last_seen.clone()),
                client.metadata.callback_url.clone().map(SqlStorageValue::String).unwrap_or(SqlStorageValue::Null),
            ]),
        )?;

        Ok(())
    }

    /// Delete client from SQLite
    fn delete_client(&self, client_id: &str) -> Result<()> {
        self.ensure_initialized()?;
        let sql = self.state.storage().sql();

        sql.exec(
            "DELETE FROM clients WHERE client_id = ?",
            Some(vec![SqlStorageValue::String(client_id.to_string())]),
        )?;

        Ok(())
    }

    /// Register client in D1 for public path routing (client_id -> user_id mapping)
    async fn register_client_in_d1(&self, client: &Client) -> Result<()> {
        let db = self.env.d1("DB")?;
        let stmt = db.prepare(
            "INSERT OR REPLACE INTO clients (client_id, user_id, hostname, project, connected_at, last_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        );
        stmt.bind(&[
            client.id.clone().into(),
            client.user_id.clone().into(),
            client.metadata.hostname.clone().into(),
            client.metadata.project.clone().into(),
            client.connected_at.clone().into(),
            client.last_seen.clone().into(),
        ])?.run().await?;
        Ok(())
    }

    /// Unregister client from D1
    async fn unregister_client_from_d1(&self, client_id: &str) -> Result<()> {
        let db = self.env.d1("DB")?;
        let stmt = db.prepare("DELETE FROM clients WHERE client_id = ?1");
        stmt.bind(&[client_id.into()])?.run().await?;
        Ok(())
    }

    /// Load all clients from SQLite (for recovery after hibernation)
    fn load_clients_from_sqlite(&self) -> Result<Vec<Client>> {
        self.ensure_initialized()?;
        let sql = self.state.storage().sql();

        let cursor = sql.exec(
            "SELECT client_id, user_id, hostname, project, status, last_activity, connected_at, last_seen, callback_url FROM clients",
            None,
        )?;

        // Use typed deserialization
        let rows: Vec<ClientRow> = cursor.to_array()?;
        let mut clients = Vec::new();

        for row_value in rows {
            let status = match row_value.status.as_str() {
                "active" => ClientStatus::Active,
                "busy" => ClientStatus::Busy,
                "disconnected" => ClientStatus::Disconnected,
                _ => ClientStatus::Idle,
            };

            clients.push(Client {
                id: row_value.client_id,
                user_id: row_value.user_id,
                metadata: ClientMetadata {
                    hostname: row_value.hostname,
                    project: row_value.project,
                    status,
                    last_activity: row_value.last_activity,
                    callback_url: row_value.callback_url,
                },
                connected_at: row_value.connected_at,
                last_seen: row_value.last_seen,
            });
        }

        Ok(clients)
    }

    /// Restore in-memory state from SQLite and WebSockets after hibernation
    fn restore_state(&self) -> Result<()> {
        // Get active WebSockets from hibernation API
        let websockets = self.state.get_websockets();

        // Load client data from SQLite
        let stored_clients = self.load_clients_from_sqlite()?;
        let client_map: HashMap<String, Client> = stored_clients
            .into_iter()
            .map(|c| (c.id.clone(), c))
            .collect();

        // Match WebSockets with their client data using tags
        let mut clients = self.clients.borrow_mut();
        let mut browsers = self.browsers.borrow_mut();

        for ws in websockets {
            let tags = self.state.get_tags(&ws);
            if tags.iter().any(|t| t == "browser") {
                browsers.push(ws);
            } else if let Some(client_id) = tags.first() {
                if let Some(client) = client_map.get(client_id) {
                    clients.insert(
                        client_id.clone(),
                        ClientConnection {
                            websocket: ws,
                            client: client.clone(),
                        },
                    );
                }
            }
        }

        Ok(())
    }

    async fn handle_websocket(&self, req: Request) -> Result<Response> {
        // Get the upgrade header
        let upgrade = req.headers().get("Upgrade")?;
        if upgrade.as_deref() != Some("websocket") {
            return Response::error("Expected websocket", 426);
        }

        // Parse query parameters
        let url = req.url()?;
        let is_browser = url.query_pairs().any(|(k, v)| k == "type" && v == "browser");
        let client_id: Option<String> = url
            .query_pairs()
            .find(|(k, _)| k == "client_id")
            .map(|(_, v)| v.to_string());

        let pair = WebSocketPair::new()?;
        let server = pair.server;
        let client = pair.client;

        // Use hibernation API for WebSocket acceptance with tags for recovery
        // Tags allow us to identify WebSockets after hibernation
        if is_browser {
            self.state.accept_websocket_with_tags(&server, &["browser"]);
        } else if let Some(id) = client_id {
            // Tag client WebSocket with its client_id for hibernation recovery
            self.state.accept_websocket_with_tags(&server, &[&id]);
        } else {
            // Legacy: no client_id provided (shouldn't happen with updated claudecodeui)
            self.state.accept_web_socket(&server);
        }

        Response::from_websocket(client)
    }

    /// Ensure state is restored after hibernation
    fn ensure_state_restored(&self) -> Result<()> {
        // Only restore if clients map is empty but we have websockets
        if self.clients.borrow().is_empty() {
            let websockets = self.state.get_websockets();
            if !websockets.is_empty() {
                self.restore_state()?;
            }
        }
        Ok(())
    }

    async fn handle_message(&self, ws: &WebSocket, text: &str) -> Result<()> {
        // Restore state if waking from hibernation
        let _ = self.ensure_state_restored();

        let msg: WsMessage = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                let error = WsMessage::Error {
                    message: format!("Invalid message format: {}", e),
                };
                if let Ok(json) = serde_json::to_string(&error) {
                    let _ = ws.send_with_str(&json);
                }
                return Ok(());
            }
        };

        match msg {
            WsMessage::Register {
                client_id,
                user_token: _,
                metadata,
            } => {
                // Create client
                let user_id = self.state.id().to_string();
                let client = Client::new(client_id.clone(), user_id, metadata);

                // Save to SQLite for persistence
                let _ = self.save_client(&client);

                // Register in D1 for public path routing (allows anonymous access to manifest.json etc.)
                if let Err(e) = self.register_client_in_d1(&client).await {
                    console_log!("Failed to register client in D1: {:?}", e);
                }

                // Tag the WebSocket with the client ID for hibernation recovery
                // Note: We need to re-accept with tags, but that's not possible after accept
                // So we track the mapping in SQLite instead

                // Send registration success response
                let registered = WsMessage::Registered {
                    success: true,
                    message: None,
                };
                if let Ok(json) = serde_json::to_string(&registered) {
                    let _ = ws.send_with_str(&json);
                }

                // Broadcast update to browsers
                if let Ok(json) = serde_json::to_string(&WsMessage::ClientUpdate {
                    client: client.clone(),
                }) {
                    self.broadcast_to_browsers(&json);
                }

                // Store connection in memory
                self.clients.borrow_mut().insert(
                    client_id,
                    ClientConnection {
                        websocket: ws.clone(),
                        client,
                    },
                );
            }

            WsMessage::StatusUpdate { client_id, status } => {
                // Get client, update, and extract data
                let mut clients = self.clients.borrow_mut();
                if let Some(conn) = clients.get_mut(&client_id) {
                    conn.client.update_status(status);
                    conn.client.update_last_seen();
                    let client_clone = conn.client.clone();

                    // Update SQLite
                    let _ = self.save_client(&client_clone);

                    // Broadcast to browsers
                    if let Ok(json) =
                        serde_json::to_string(&WsMessage::ClientUpdate { client: client_clone })
                    {
                        drop(clients); // Release borrow before broadcasting
                        self.broadcast_to_browsers(&json);
                    }
                }
            }

            WsMessage::Ping { client_id } => {
                let client_to_save = {
                    let mut clients = self.clients.borrow_mut();
                    if let Some(conn) = clients.get_mut(&client_id) {
                        conn.client.update_last_seen();
                        Some(conn.client.clone())
                    } else {
                        None
                    }
                };

                // Update last_seen in SQLite periodically (on pings)
                if let Some(client) = client_to_save {
                    let _ = self.save_client(&client);
                }

                let pong = WsMessage::Pong { client_id };
                if let Ok(json) = serde_json::to_string(&pong) {
                    let _ = ws.send_with_str(&json);
                }
            }

            WsMessage::GetClients => {
                // This is a browser requesting the client list
                // Add it to browsers if not already there
                {
                    let mut browsers = self.browsers.borrow_mut();
                    if !browsers.contains(ws) {
                        browsers.push(ws.clone());
                    }
                }

                // Get clients with active WebSocket connections from memory
                let active_client_ids: std::collections::HashSet<String> = self
                    .clients
                    .borrow()
                    .keys()
                    .cloned()
                    .collect();

                let mut clients: Vec<Client> = self
                    .clients
                    .borrow()
                    .values()
                    .map(|c| c.client.clone())
                    .collect();

                // Check SQLite for any clients that might be stale
                // Mark them as disconnected if their WebSocket is not in memory
                if let Ok(stored) = self.load_clients_from_sqlite() {
                    for mut stored_client in stored {
                        if !active_client_ids.contains(&stored_client.id) {
                            // Client in SQLite but not in memory - mark as disconnected
                            if !matches!(stored_client.metadata.status, ClientStatus::Disconnected) {
                                stored_client.update_status(ClientStatus::Disconnected);
                                let _ = self.save_client(&stored_client);
                            }
                            clients.push(stored_client);
                        }
                    }
                }

                let response = WsMessage::ClientList { clients };
                if let Ok(json) = serde_json::to_string(&response) {
                    let _ = ws.send_with_str(&json);
                }
            }

            WsMessage::ConnectClient { client_id } => {
                // Browser is requesting to connect to a specific claudecodeui client
                // Only clients with active WebSocket connections can be connected to
                let client_opt = {
                    let clients = self.clients.borrow();
                    clients.get(&client_id).map(|conn| conn.client.clone())
                };

                let response = if let Some(c) = client_opt {
                    // Client has an active WebSocket connection
                    WsMessage::ConnectResponse {
                        success: true,
                        client_id: client_id.clone(),
                        url: None,
                        message: Some(format!(
                            "Connected to '{}'. Use forward_to_client to send commands.",
                            client_id
                        )),
                    }
                } else {
                    // Check SQLite - if client exists but no WebSocket, mark as disconnected
                    let client_in_sqlite = self.load_clients_from_sqlite()
                        .ok()
                        .and_then(|clients| clients.into_iter().find(|c| c.id == client_id));

                    if let Some(mut stale_client) = client_in_sqlite {
                        // Mark as disconnected if not already
                        if !matches!(stale_client.metadata.status, ClientStatus::Disconnected) {
                            stale_client.update_status(ClientStatus::Disconnected);
                            let _ = self.save_client(&stale_client);

                            // Broadcast status change to browsers
                            if let Ok(json) = serde_json::to_string(&WsMessage::ClientUpdate {
                                client: stale_client,
                            }) {
                                self.broadcast_to_browsers(&json);
                            }
                        }

                        WsMessage::ConnectResponse {
                            success: false,
                            client_id: client_id.clone(),
                            url: None,
                            message: Some("Client is offline (no active connection)".to_string()),
                        }
                    } else {
                        WsMessage::ConnectResponse {
                            success: false,
                            client_id: client_id.clone(),
                            url: None,
                            message: Some("Client not found".to_string()),
                        }
                    }
                };

                if let Ok(json) = serde_json::to_string(&response) {
                    let _ = ws.send_with_str(&json);
                }
            }

            WsMessage::ForwardToClient {
                client_id,
                request_id,
                action,
                payload,
            } => {
                // Browser wants to forward a request to a claudecodeui client
                // Find the client's WebSocket
                let client_ws_opt = {
                    let clients = self.clients.borrow();
                    clients.get(&client_id).map(|conn| conn.websocket.clone())
                };

                if let Some(client_ws) = client_ws_opt {
                    // Track this pending request so we can route responses back
                    self.pending_requests.borrow_mut().insert(
                        request_id.clone(),
                        PendingRequest {
                            client_id: client_id.clone(),
                            browser_ws: ws.clone(),
                        },
                    );

                    // Forward as user_request to claudecodeui
                    let user_request = WsMessage::UserRequest {
                        request_id,
                        action,
                        payload,
                    };
                    if let Ok(json) = serde_json::to_string(&user_request) {
                        let _ = client_ws.send_with_str(&json);
                    }
                } else {
                    // Client WebSocket not in memory - check if they're in SQLite
                    // This can happen after hibernation: SQLite shows "connected" but
                    // the actual WebSocket was lost. Mark them as disconnected.
                    let client_in_sqlite = self.load_clients_from_sqlite()
                        .ok()
                        .and_then(|clients| clients.into_iter().find(|c| c.id == client_id));

                    if let Some(mut stale_client) = client_in_sqlite {
                        // Client was in SQLite but WebSocket is gone - mark as disconnected
                        stale_client.update_status(ClientStatus::Disconnected);
                        let _ = self.save_client(&stale_client);

                        // Broadcast status change to browsers
                        if let Ok(json) = serde_json::to_string(&WsMessage::ClientUpdate {
                            client: stale_client,
                        }) {
                            self.broadcast_to_browsers(&json);
                        }

                        // Send error back to browser
                        let error = WsMessage::ForwardedResponse {
                            client_id,
                            request_id,
                            data: serde_json::json!({
                                "error": true,
                                "message": "Client is offline (connection lost after hibernation)"
                            }),
                            complete: true,
                        };
                        if let Ok(json) = serde_json::to_string(&error) {
                            let _ = ws.send_with_str(&json);
                        }
                    } else {
                        // Client not found at all
                        let error = WsMessage::ForwardedResponse {
                            client_id,
                            request_id,
                            data: serde_json::json!({
                                "error": true,
                                "message": "Client not found"
                            }),
                            complete: true,
                        };
                        if let Ok(json) = serde_json::to_string(&error) {
                            let _ = ws.send_with_str(&json);
                        }
                    }
                }
            }

            WsMessage::ResponseChunk { request_id, data } => {
                // Response chunk from claudecodeui - route back to browser
                let pending = self.pending_requests.borrow();
                if let Some(req) = pending.get(&request_id) {
                    let response = WsMessage::ForwardedResponse {
                        client_id: req.client_id.clone(),
                        request_id: request_id.clone(),
                        data,
                        complete: false,
                    };
                    if let Ok(json) = serde_json::to_string(&response) {
                        let _ = req.browser_ws.send_with_str(&json);
                    }
                }
            }

            WsMessage::ResponseComplete { request_id, data } => {
                // Response complete from claudecodeui - route back to browser and clean up
                let pending_req = self.pending_requests.borrow_mut().remove(&request_id);
                if let Some(req) = pending_req {
                    let response = WsMessage::ForwardedResponse {
                        client_id: req.client_id,
                        request_id,
                        data: data.unwrap_or(serde_json::json!({"complete": true})),
                        complete: true,
                    };
                    if let Ok(json) = serde_json::to_string(&response) {
                        let _ = req.browser_ws.send_with_str(&json);
                    }
                }
            }

            WsMessage::HttpProxyResponse {
                request_id,
                status,
                headers,
                body,
            } => {
                // HTTP proxy response from claudecodeui - resolve the pending request
                self.handle_http_proxy_response(&request_id, status, headers, body);
            }

            _ => {
                // Other message types not handled here (UserRequest, ForwardedResponse are outbound only)
            }
        }

        Ok(())
    }

    async fn handle_close(&self, ws: &WebSocket) {
        // Remove from browsers list
        self.browsers.borrow_mut().retain(|b| b != ws);

        // Remove from clients and broadcast disconnection
        let disconnected_id = {
            let clients = self.clients.borrow();
            clients
                .iter()
                .find(|(_, conn)| &conn.websocket == ws)
                .map(|(id, _)| id.clone())
        };

        if let Some(client_id) = disconnected_id {
            self.clients.borrow_mut().remove(&client_id);

            // Remove from SQLite
            let _ = self.delete_client(&client_id);

            // Remove from D1 (for public path routing)
            if let Err(e) = self.unregister_client_from_d1(&client_id).await {
                console_log!("Failed to unregister client from D1: {:?}", e);
            }

            // Broadcast disconnection to browsers
            if let Ok(msg) =
                serde_json::to_string(&WsMessage::ClientDisconnected { client_id })
            {
                self.broadcast_to_browsers(&msg);
            }
        }
    }

    fn get_clients_json(&self) -> Result<Response> {
        // Restore state if waking from hibernation
        let _ = self.ensure_state_restored();

        // Get clients with active WebSocket connections from memory
        let active_client_ids: std::collections::HashSet<String> = self
            .clients
            .borrow()
            .keys()
            .cloned()
            .collect();

        let mut clients: Vec<Client> = self
            .clients
            .borrow()
            .values()
            .map(|c| c.client.clone())
            .collect();

        // Check SQLite for any clients that might be stale
        // Mark them as disconnected if their WebSocket is not in memory
        if let Ok(stored) = self.load_clients_from_sqlite() {
            for mut stored_client in stored {
                if !active_client_ids.contains(&stored_client.id) {
                    // Client in SQLite but not in memory - mark as disconnected
                    if !matches!(stored_client.metadata.status, ClientStatus::Disconnected) {
                        stored_client.update_status(ClientStatus::Disconnected);
                        let _ = self.save_client(&stored_client);
                    }
                    clients.push(stored_client);
                }
            }
        }

        Response::from_json(&clients)
    }

    fn broadcast_to_browsers(&self, message: &str) {
        for browser in self.browsers.borrow().iter() {
            let _ = browser.send_with_str(message);
        }
    }

    /// Disconnect a specific client by ID
    async fn disconnect_client(&self, client_id: &str) -> Result<Response> {
        // Restore state if needed
        let _ = self.ensure_state_restored();

        // Find and remove the client
        let connection = self.clients.borrow_mut().remove(client_id);

        if let Some(conn) = connection {
            // Send disconnect command to the client
            let disconnect_msg = WsMessage::Error {
                message: "Disconnected by user".to_string(),
            };
            if let Ok(json) = serde_json::to_string(&disconnect_msg) {
                let _ = conn.websocket.send_with_str(&json);
            }
            // Close the WebSocket
            let _ = conn.websocket.close(Some(1000), Some("Disconnected by user"));

            // Delete from SQLite
            let _ = self.delete_client(client_id);

            // Delete from D1 (for public path routing)
            if let Err(e) = self.unregister_client_from_d1(client_id).await {
                console_log!("Failed to unregister client from D1: {:?}", e);
            }

            // Broadcast disconnection to browsers
            if let Ok(msg) =
                serde_json::to_string(&WsMessage::ClientDisconnected {
                    client_id: client_id.to_string(),
                })
            {
                self.broadcast_to_browsers(&msg);
            }

            Response::ok("Client disconnected")
        } else {
            Response::error("Client not found", 404)
        }
    }

    /// Handle HTTP proxy requests to claudecodeui instances via WebSocket
    async fn handle_proxy(&self, mut req: Request, client_id: &str) -> Result<Response> {
        // Restore state if waking from hibernation
        let _ = self.ensure_state_restored();

        // Parse the proxy request from the body
        let body_text = req.text().await?;
        let proxy_req: ProxyRequest = serde_json::from_str(&body_text)
            .map_err(|e| Error::RustError(format!("Invalid proxy request: {}", e)))?;

        // Find the client's WebSocket connection
        let client_ws = {
            let clients = self.clients.borrow();
            clients.get(client_id).map(|conn| conn.websocket.clone())
        };

        let client_ws = match client_ws {
            Some(ws) => ws,
            None => {
                return Response::from_json(&ProxyResponse {
                    status: 503,
                    headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                    body: r#"{"error": "Client not connected"}"#.to_string(),
                });
            }
        };

        // Generate a unique request ID
        let request_id = generate_request_id();

        // Create oneshot channel for response
        let (sender, receiver) = oneshot::channel::<ProxyResponse>();

        // Store the sender in pending_proxy_requests
        {
            let mut pending = self.pending_proxy_requests.borrow_mut();
            pending.insert(request_id.clone(), sender);
        }

        // Build and send the HttpProxyRequest message
        // Include proxy_base so claudecodeui can rewrite URLs in responses
        let proxy_base = format!("/clients/{}/proxy", client_id);
        let proxy_msg = WsMessage::HttpProxyRequest {
            request_id: request_id.clone(),
            method: proxy_req.method,
            path: proxy_req.path,
            headers: proxy_req.headers,
            body: proxy_req.body,
            query: proxy_req.query,
            proxy_base: Some(proxy_base),
        };

        if let Ok(msg_json) = serde_json::to_string(&proxy_msg) {
            if client_ws.send_with_str(&msg_json).is_err() {
                // Remove from pending and return error
                self.pending_proxy_requests.borrow_mut().remove(&request_id);
                return Response::from_json(&ProxyResponse {
                    status: 502,
                    headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                    body: r#"{"error": "Failed to send request to client"}"#.to_string(),
                });
            }
        }

        // Wait for response with timeout
        // Note: In WASM, we use wasm_bindgen_futures and a JavaScript timeout
        let timeout_ms = 30000; // 30 seconds
        let timeout_future = async {
            let promise = js_sys::Promise::new(&mut |resolve, _| {
                let window = js_sys::global();
                let _ = js_sys::Reflect::get(&window, &JsValue::from_str("setTimeout"))
                    .and_then(|set_timeout| {
                        let set_timeout_fn: js_sys::Function = set_timeout.into();
                        set_timeout_fn.call2(&JsValue::NULL, &resolve, &JsValue::from(timeout_ms))
                    });
            });
            wasm_bindgen_futures::JsFuture::from(promise).await
        };

        // Use futures::select to race between response and timeout
        use futures::future::{select, Either};
        use std::pin::pin;

        let receiver_future = pin!(receiver);
        let timeout_future = pin!(timeout_future);

        let result = select(receiver_future, timeout_future).await;

        match result {
            Either::Left((Ok(proxy_response), _)) => {
                // Got response from client
                Response::from_json(&proxy_response)
            }
            Either::Left((Err(_), _)) => {
                // Channel was dropped (client disconnected?)
                self.pending_proxy_requests.borrow_mut().remove(&request_id);
                Response::from_json(&ProxyResponse {
                    status: 502,
                    headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                    body: r#"{"error": "Client disconnected before responding"}"#.to_string(),
                })
            }
            Either::Right((_, _)) => {
                // Timeout
                self.pending_proxy_requests.borrow_mut().remove(&request_id);
                Response::from_json(&ProxyResponse {
                    status: 504,
                    headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                    body: r#"{"error": "Request timed out"}"#.to_string(),
                })
            }
        }
    }

    /// Handle HttpProxyResponse from claudecodeui
    fn handle_http_proxy_response(&self, request_id: &str, status: u16, headers: Vec<(String, String)>, body: String) {
        let mut pending = self.pending_proxy_requests.borrow_mut();
        if let Some(sender) = pending.remove(request_id) {
            let response = ProxyResponse {
                status,
                headers,
                body,
            };
            let _ = sender.send(response);
        }
    }
}

/// Generate a unique request ID
fn generate_request_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).unwrap_or_default();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}
