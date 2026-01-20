use serde::Deserialize;
use worker::*;

use crate::auth::AuthMiddleware;
use crate::models::{parse_token, verify_token};

/// Row for token validation query
#[derive(Debug, Deserialize)]
struct TokenRow {
    user_id: String,
    token_hash: String,
}

/// WebSocket upgrade handler - routes to appropriate Durable Object
pub async fn websocket_upgrade(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Note: Cloudflare handles WebSocket upgrade at edge - we just need to forward to DO
    // The DO will handle the actual WebSocket connection via WebSocketPair

    let url = req.url()?;
    let params: std::collections::HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    // Determine connection type
    let is_browser = params.get("type").map(|t| t == "browser").unwrap_or(false);

    // For browser connections, require authentication via session cookie
    if is_browser {
        let user = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
            Ok(user) => user,
            Err(redirect) => return Ok(redirect),
        };

        // Forward to user's Durable Object
        let namespace = ctx.env.durable_object("USER_HUB")?;
        let id = namespace.id_from_name(&user.id)?;
        let stub = id.get_stub()?;

        // Forward with WebSocket upgrade headers for DO
        let headers = Headers::new();
        headers.set("Upgrade", "websocket")?;

        let mut init = RequestInit::new();
        init.with_method(Method::Get);
        init.with_headers(headers);

        let do_req = Request::new_with_init("https://do/ws?type=browser", &init)?;
        stub.fetch_with_request(do_req).await
    } else {
        // claudecodeui connection - authenticate via token
        let full_token = params.get("token").ok_or("Missing token parameter")?;

        // Parse and validate token
        let (token_id, raw_token) = match parse_token(full_token) {
            Some(parts) => parts,
            None => return Response::error("Invalid token format", 401),
        };

        // Look up token in D1
        let db = ctx.env.d1("DB")?;
        let token_result = db
            .prepare(
                "SELECT user_id, token_hash FROM client_tokens WHERE id = ?1 AND revoked_at IS NULL",
            )
            .bind(&[token_id.clone().into()])?
            .first::<TokenRow>(None)
            .await?;

        let token_row = match token_result {
            Some(row) => row,
            None => return Response::error("Token not found or revoked", 401),
        };

        // Verify token hash
        if !verify_token(&raw_token, &token_row.token_hash) {
            return Response::error("Invalid token", 401);
        }

        // Update last_used timestamp (fire and forget)
        let _ = db
            .prepare("UPDATE client_tokens SET last_used = CURRENT_TIMESTAMP WHERE id = ?1")
            .bind(&[token_id.into()])?
            .run()
            .await;

        // Forward to user's Durable Object
        let namespace = ctx.env.durable_object("USER_HUB")?;
        let id = namespace.id_from_name(&token_row.user_id)?;
        let stub = id.get_stub()?;

        // Forward with WebSocket upgrade headers for DO
        let headers = Headers::new();
        headers.set("Upgrade", "websocket")?;

        let mut init = RequestInit::new();
        init.with_method(Method::Get);
        init.with_headers(headers);

        // Include client_id in the DO request URL for hibernation-aware tagging
        let client_id = params.get("client_id").cloned().unwrap_or_default();
        let do_url = format!("https://do/ws?client_id={}", client_id);
        let do_req = Request::new_with_init(&do_url, &init)?;
        stub.fetch_with_request(do_req).await
    }
}
