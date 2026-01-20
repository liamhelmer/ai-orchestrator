mod middleware;

pub use middleware::AuthMiddleware;

use serde::{Deserialize, Serialize};
use worker::*;

const GITHUB_AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_URL: &str = "https://api.github.com/user";
const GITHUB_ORGS_URL: &str = "https://api.github.com/user/orgs";

#[derive(Debug, Serialize, Deserialize)]
struct GitHubUser {
    id: i64,
    login: String,
    email: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GitHubOrg {
    id: i64,
    login: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    scope: String,
}

/// Start GitHub OAuth flow
pub async fn start_oauth(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let client_id = ctx.env.var("GITHUB_CLIENT_ID")?.to_string();
    let redirect_uri = get_redirect_uri(&req)?;

    // Generate state for CSRF protection
    let state = generate_state();

    // Store state in cookie for validation
    let auth_url = format!(
        "{}?client_id={}&redirect_uri={}&scope=read:org%20read:user%20user:email&state={}",
        GITHUB_AUTHORIZE_URL,
        client_id,
        url_encode(&redirect_uri),
        state
    );

    let headers = Headers::new();
    headers.set("Location", &auth_url)?;
    headers.set(
        "Set-Cookie",
        &format!(
            "oauth_state={}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=600",
            state
        ),
    )?;

    Response::empty()
        .map(|r| r.with_status(302))
        .map(|r| r.with_headers(headers))
}

/// Handle GitHub OAuth callback
pub async fn handle_callback(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    match handle_callback_inner(req, ctx).await {
        Ok(response) => Ok(response),
        Err(e) => {
            console_log!("OAuth callback error: {:?}", e);
            Response::error(format!("OAuth error: {:?}", e), 500)
        }
    }
}

async fn handle_callback_inner(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    console_log!("OAuth callback started");

    let url = req.url()?;
    let params: std::collections::HashMap<String, String> = url
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    console_log!("Params parsed, verifying state");

    // Verify state matches
    let state = params.get("state").ok_or("Missing state parameter")?;
    let cookie_state = match get_cookie(&req, "oauth_state") {
        Ok(s) => s,
        Err(e) => {
            console_log!("Failed to get oauth_state cookie: {:?}", e);
            return Response::error("Cookie not found", 400);
        }
    };

    if state != &cookie_state {
        console_log!("State mismatch: {} vs {}", state, cookie_state);
        return Response::error("Invalid state parameter", 400);
    }

    console_log!("State verified, exchanging code for token");

    // Exchange code for token
    let code = params.get("code").ok_or("Missing code parameter")?;
    let client_id = ctx.env.var("GITHUB_CLIENT_ID")?.to_string();
    let client_secret = ctx.env.secret("GITHUB_CLIENT_SECRET")?.to_string();
    let redirect_uri = get_redirect_uri(&req)?;

    let token = match exchange_code_for_token(&client_id, &client_secret, code, &redirect_uri).await {
        Ok(t) => t,
        Err(e) => {
            console_log!("Token exchange failed: {:?}", e);
            return Response::error(format!("Token exchange failed: {:?}", e), 500);
        }
    };

    console_log!("Token obtained, getting user info");

    // Get user info
    let github_user = match get_github_user(&token).await {
        Ok(u) => u,
        Err(e) => {
            console_log!("Failed to get GitHub user: {:?}", e);
            return Response::error(format!("GitHub user fetch failed: {:?}", e), 500);
        }
    };

    console_log!("GitHub user: {}", github_user.login);

    // Verify org/user/team restrictions
    let allowed_orgs: Vec<String> = ctx
        .env
        .var("ALLOWED_ORGS")?
        .to_string()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().to_string())
        .collect();

    let allowed_users: Vec<String> = ctx
        .env
        .var("ALLOWED_USERS")?
        .to_string()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().to_string())
        .collect();

    // Check if user is allowed
    let is_allowed = if allowed_users.is_empty() && allowed_orgs.is_empty() {
        true // No restrictions configured
    } else {
        let user_allowed = allowed_users.contains(&github_user.login);
        let org_allowed = if !allowed_orgs.is_empty() {
            check_org_membership(&token, &allowed_orgs).await?
        } else {
            false
        };
        user_allowed || org_allowed
    };

    if !is_allowed {
        return Response::error("Access denied: not authorized", 403);
    }

    // Create user record (for new users)
    let new_user = crate::models::User::new(github_user.id, github_user.login.clone(), github_user.email.clone());

    console_log!("Processing user: {} (github_id: {})", new_user.github_login, new_user.github_id);

    // Store user and session in D1
    let db = ctx.env.d1("DB")?;

    // Upsert user
    let user_result = db.prepare(
        "INSERT INTO users (id, github_id, github_login, email, last_login)
         VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
         ON CONFLICT(github_id) DO UPDATE SET
         github_login = excluded.github_login,
         email = excluded.email,
         last_login = CURRENT_TIMESTAMP"
    )
    .bind(&[
        new_user.id.clone().into(),
        wasm_bindgen::JsValue::from_f64(new_user.github_id as f64),  // D1 doesn't support bigint
        new_user.github_login.clone().into(),
        github_user.email.clone().map(|e| e.into()).unwrap_or(wasm_bindgen::JsValue::NULL),
    ])?
    .run()
    .await;

    if let Err(e) = &user_result {
        console_log!("Error upserting user: {:?}", e);
        return Response::error(format!("Database error (user): {:?}", e), 500);
    }
    console_log!("User upserted successfully");

    // Fetch the actual user ID (may be different for returning users)
    #[derive(serde::Deserialize)]
    struct UserIdRow {
        id: String,
    }
    let user_id_result = db.prepare(
        "SELECT id FROM users WHERE github_id = ?1"
    )
    .bind(&[wasm_bindgen::JsValue::from_f64(github_user.id as f64)])?
    .first::<UserIdRow>(None)
    .await?;

    let actual_user_id = match user_id_result {
        Some(row) => row.id,
        None => {
            console_log!("User not found after upsert");
            return Response::error("User not found after creation", 500);
        }
    };
    console_log!("Using user_id: {}", actual_user_id);

    // Create session with the actual user ID
    let session = crate::models::Session::new(actual_user_id.clone(), 24 * 7); // 1 week

    // Insert session
    let session_result = db.prepare(
        "INSERT INTO sessions (id, user_id, expires_at) VALUES (?1, ?2, ?3)"
    )
    .bind(&[
        session.id.clone().into(),
        actual_user_id.into(),
        session.expires_at.clone().into(),
    ])?
    .run()
    .await;

    if let Err(e) = &session_result {
        console_log!("Error inserting session: {:?}", e);
        return Response::error(format!("Database error (session): {:?}", e), 500);
    }
    console_log!("Session created successfully");

    // Redirect to dashboard with session ID cookie (just the ID, not full data)
    let headers = Headers::new();
    headers.set("Location", "/dashboard")?;
    headers.set(
        "Set-Cookie",
        &format!(
            "session={}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={}",
            session.id,
            7 * 24 * 60 * 60 // 1 week in seconds
        ),
    )?;

    Response::empty()
        .map(|r| r.with_status(302))
        .map(|r| r.with_headers(headers))
}

/// Logout and clear session
pub async fn logout(_req: Request, _ctx: RouteContext<()>) -> Result<Response> {
    // Clear session cookie
    let headers = Headers::new();
    headers.set("Location", "/")?;
    headers.set(
        "Set-Cookie",
        "session=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0",
    )?;

    Response::empty()
        .map(|r| r.with_status(302))
        .map(|r| r.with_headers(headers))
}

async fn exchange_code_for_token(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<String> {
    let body = format!(
        "client_id={}&client_secret={}&code={}&redirect_uri={}",
        client_id, client_secret, code, redirect_uri
    );

    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    init.with_body(Some(wasm_bindgen::JsValue::from_str(&body)));

    let headers = Headers::new();
    headers.set("Accept", "application/json")?;
    headers.set("Content-Type", "application/x-www-form-urlencoded")?;
    init.with_headers(headers);

    let request = Request::new_with_init(GITHUB_TOKEN_URL, &init)?;
    let mut response = Fetch::Request(request).send().await?;
    let token_response: TokenResponse = response.json().await?;

    Ok(token_response.access_token)
}

async fn get_github_user(token: &str) -> Result<GitHubUser> {
    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {}", token))?;
    headers.set("User-Agent", "AI-Orchestrator")?;
    headers.set("Accept", "application/json")?;

    let mut init = RequestInit::new();
    init.with_method(Method::Get);
    init.with_headers(headers);

    let request = Request::new_with_init(GITHUB_USER_URL, &init)?;
    let mut response = Fetch::Request(request).send().await?;
    response.json().await
}

async fn check_org_membership(token: &str, allowed_orgs: &[String]) -> Result<bool> {
    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {}", token))?;
    headers.set("User-Agent", "AI-Orchestrator")?;
    headers.set("Accept", "application/json")?;

    let mut init = RequestInit::new();
    init.with_method(Method::Get);
    init.with_headers(headers);

    let request = Request::new_with_init(GITHUB_ORGS_URL, &init)?;
    let mut response = Fetch::Request(request).send().await?;
    let orgs: Vec<GitHubOrg> = response.json().await?;

    Ok(orgs.iter().any(|org| allowed_orgs.contains(&org.login)))
}

fn get_redirect_uri(req: &Request) -> Result<String> {
    let url = req.url()?;
    let host = url.host_str().ok_or("Missing host")?;
    // Always use HTTPS for Cloudflare Workers (required for Secure cookies)
    let scheme = if host.ends_with(".workers.dev") || host.ends_with(".pages.dev") {
        "https"
    } else {
        url.scheme()
    };
    let port = url.port().map(|p| format!(":{}", p)).unwrap_or_default();
    Ok(format!(
        "{}://{}{}/auth/github/callback",
        scheme, host, port
    ))
}

fn generate_state() -> String {
    use getrandom::getrandom;
    let mut bytes = [0u8; 16];
    getrandom(&mut bytes).expect("Failed to generate random bytes");
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn get_cookie(req: &Request, name: &str) -> Result<String> {
    let cookie_header = req.headers().get("Cookie")?.unwrap_or_default();
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{}=", name)) {
            return Ok(value.to_string());
        }
    }
    Err("Cookie not found".into())
}

fn url_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}
