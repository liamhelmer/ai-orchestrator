use serde::Deserialize;
use worker::*;

use crate::auth::AuthMiddleware;
#[allow(unused_imports)]
use crate::models::{hash_token, parse_token, verify_token, ClientToken, TokenCreated, TokenInfo};
use crate::templates;

/// Request to create a new token
#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    pub name: String,
}

/// D1 row for tokens
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TokenRow {
    id: String,
    user_id: String,
    name: String,
    created_at: String,
    last_used: Option<String>,
    revoked_at: Option<String>,
}

/// List all tokens for the authenticated user
pub async fn list_tokens(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Require authentication
    let user = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(user) => user,
        Err(response) => return Ok(response),
    };

    let db = ctx.env.d1("DB")?;

    let tokens = db
        .prepare(
            "SELECT id, user_id, name, created_at, last_used, revoked_at
             FROM client_tokens
             WHERE user_id = ?1
             ORDER BY created_at DESC",
        )
        .bind(&[user.id.into()])?
        .all()
        .await?;

    let rows: Vec<TokenRow> = tokens.results()?;
    let token_infos: Vec<TokenInfo> = rows
        .into_iter()
        .map(|row| TokenInfo {
            id: row.id,
            name: row.name,
            created_at: row.created_at,
            last_used: row.last_used,
            is_revoked: row.revoked_at.is_some(),
        })
        .collect();

    Response::from_json(&token_infos)
}

/// Create a new token for the authenticated user
pub async fn create_token(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Require authentication
    let user = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(user) => user,
        Err(response) => return Ok(response),
    };

    // Parse request body
    let body: CreateTokenRequest = req.json().await?;

    // Generate token
    let (token, raw_token) = ClientToken::new(user.id.clone(), body.name.clone());
    let token_hash = hash_token(&raw_token.split('_').last().unwrap_or(&raw_token));

    // Store in D1
    let db = ctx.env.d1("DB")?;
    db.prepare(
        "INSERT INTO client_tokens (id, user_id, name, token_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&[
        token.id.clone().into(),
        user.id.into(),
        body.name.into(),
        token_hash.into(),
        token.created_at.into(),
    ])?
    .run()
    .await?;

    // Return the token (only time it's shown)
    let response = TokenCreated {
        id: token.id,
        name: token.name,
        token: raw_token,
    };

    Response::from_json(&response)
}

/// Revoke a token
pub async fn revoke_token(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Require authentication
    let user = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(user) => user,
        Err(response) => return Ok(response),
    };

    let token_id = ctx.param("id").ok_or("Missing token ID")?;

    let db = ctx.env.d1("DB")?;

    // Update token to revoked (only if owned by user)
    let result = db
        .prepare(
            "UPDATE client_tokens
             SET revoked_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND user_id = ?2 AND revoked_at IS NULL",
        )
        .bind(&[token_id.into(), user.id.into()])?
        .run()
        .await?;

    if result.success() {
        Response::ok("Token revoked")
    } else {
        Response::error("Failed to revoke token", 500)
    }
}

/// Delete a token permanently
pub async fn delete_token(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Require authentication
    let user = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(user) => user,
        Err(response) => return Ok(response),
    };

    let token_id = ctx.param("id").ok_or("Missing token ID")?;

    let db = ctx.env.d1("DB")?;

    // Delete token (only if owned by user)
    db.prepare("DELETE FROM client_tokens WHERE id = ?1 AND user_id = ?2")
        .bind(&[token_id.into(), user.id.into()])?
        .run()
        .await?;

    Response::ok("Token deleted")
}

/// Validate a token and return user info (used internally for WebSocket auth)
#[allow(dead_code)]
pub async fn validate_token(env: &Env, full_token: &str) -> Result<Option<String>> {
    // Parse token
    let (token_id, raw_token) = match parse_token(full_token) {
        Some(parts) => parts,
        None => return Ok(None),
    };

    let token_hash = hash_token(&raw_token);

    let db = env.d1("DB")?;

    // Look up token
    let result = db
        .prepare(
            "SELECT user_id, token_hash, revoked_at
             FROM client_tokens
             WHERE id = ?1 AND revoked_at IS NULL",
        )
        .bind(&[token_id.into()])?
        .first::<TokenValidationRow>(None)
        .await?;

    match result {
        Some(row) if verify_token(&raw_token, &row.token_hash) => {
            // Update last_used timestamp
            let _ = db
                .prepare("UPDATE client_tokens SET last_used = CURRENT_TIMESTAMP WHERE id = ?1")
                .bind(&[row.id.into()])?
                .run()
                .await;

            Ok(Some(row.user_id))
        }
        _ => Ok(None),
    }
}

#[derive(Debug, Deserialize)]
struct TokenValidationRow {
    id: String,
    user_id: String,
    token_hash: String,
    #[allow(dead_code)]
    revoked_at: Option<String>,
}

// ==================== HTMX Handlers ====================

/// List tokens as HTML (HTMX partial)
pub async fn list_tokens_htmx(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Require authentication
    let user = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(user) => user,
        Err(response) => return Ok(response),
    };

    let db = ctx.env.d1("DB")?;

    let tokens = db
        .prepare(
            "SELECT id, user_id, name, created_at, last_used, revoked_at
             FROM client_tokens
             WHERE user_id = ?1
             ORDER BY created_at DESC",
        )
        .bind(&[user.id.into()])?
        .all()
        .await?;

    let rows: Vec<TokenRow> = tokens.results()?;
    let token_infos: Vec<TokenInfo> = rows
        .into_iter()
        .map(|row| TokenInfo {
            id: row.id,
            name: row.name,
            created_at: row.created_at,
            last_used: row.last_used,
            is_revoked: row.revoked_at.is_some(),
        })
        .collect();

    Response::from_html(templates::render_token_list(&token_infos))
}

/// Show the token creation modal
pub async fn show_token_modal(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Require authentication
    let _user = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(user) => user,
        Err(response) => return Ok(response),
    };

    Response::from_html(templates::render_token_modal())
}

/// Close the modal (returns empty content)
pub async fn close_token_modal(_req: Request, _ctx: RouteContext<()>) -> Result<Response> {
    Response::from_html("")
}

/// Create a new token and return success modal (HTMX)
pub async fn create_token_api(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Require authentication
    let user = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(user) => user,
        Err(response) => return Ok(response),
    };

    // Check if this is an HTMX request or API request
    let is_htmx = req.headers().get("HX-Request")?.is_some();

    // Try to parse as form data first (HTMX), then JSON (API)
    let name = if is_htmx {
        let form_data = req.form_data().await?;
        form_data
            .get("name")
            .and_then(|v| match v {
                FormEntry::Field(s) => Some(s),
                _ => None,
            })
            .ok_or("Missing name field")?
    } else {
        let body: CreateTokenRequest = req.json().await?;
        body.name
    };

    // Generate token
    let (token, raw_token) = ClientToken::new(user.id.clone(), name.clone());
    let token_hash = hash_token(&raw_token.split('_').last().unwrap_or(&raw_token));

    // Store in D1
    let db = ctx.env.d1("DB")?;
    db.prepare(
        "INSERT INTO client_tokens (id, user_id, name, token_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&[
        token.id.clone().into(),
        user.id.into(),
        name.clone().into(),
        token_hash.into(),
        token.created_at.into(),
    ])?
    .run()
    .await?;

    if is_htmx {
        // Return success modal HTML
        Response::from_html(templates::render_token_created(&raw_token, &name))
    } else {
        // Return JSON for API clients
        let response = TokenCreated {
            id: token.id,
            name: token.name,
            token: raw_token,
        };
        Response::from_json(&response)
    }
}

/// Revoke a token and return updated list (HTMX)
pub async fn revoke_token_htmx(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Require authentication
    let user = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(user) => user,
        Err(response) => return Ok(response),
    };

    let token_id = ctx.param("id").ok_or("Missing token ID")?;

    let db = ctx.env.d1("DB")?;

    // Update token to revoked (only if owned by user)
    db.prepare(
        "UPDATE client_tokens
         SET revoked_at = CURRENT_TIMESTAMP
         WHERE id = ?1 AND user_id = ?2 AND revoked_at IS NULL",
    )
    .bind(&[token_id.into(), user.id.clone().into()])?
    .run()
    .await?;

    // Check if this is an HTMX request
    let is_htmx = req.headers().get("HX-Request")?.is_some();

    if is_htmx {
        // Return updated token list
        let tokens = db
            .prepare(
                "SELECT id, user_id, name, created_at, last_used, revoked_at
                 FROM client_tokens
                 WHERE user_id = ?1
                 ORDER BY created_at DESC",
            )
            .bind(&[user.id.into()])?
            .all()
            .await?;

        let rows: Vec<TokenRow> = tokens.results()?;
        let token_infos: Vec<TokenInfo> = rows
            .into_iter()
            .map(|row| TokenInfo {
                id: row.id,
                name: row.name,
                created_at: row.created_at,
                last_used: row.last_used,
                is_revoked: row.revoked_at.is_some(),
            })
            .collect();

        Response::from_html(templates::render_token_list(&token_infos))
    } else {
        Response::ok("Token revoked")
    }
}
