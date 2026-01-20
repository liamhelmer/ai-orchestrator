use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;
use worker::*;

use crate::auth::AuthMiddleware;
use crate::models::User;

/// Proxy request to send to the Durable Object
#[derive(Debug, Serialize, Deserialize)]
pub struct ProxyRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub query: Option<String>,
}

/// Proxy response from the Durable Object
#[derive(Debug, Serialize, Deserialize)]
pub struct ProxyResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Static paths that don't require authentication (PWA resources)
const PUBLIC_PROXY_PATHS: &[&str] = &[
    "manifest.json",
    "sw.js",
    "favicon.ico",
];

/// Path prefixes that don't require authentication
const PUBLIC_PROXY_PREFIXES: &[&str] = &[
    "icons/",
];

/// Check if a path is public (no auth required)
fn is_public_path(path: &str) -> bool {
    // Normalize path (remove leading slash if present)
    let normalized = path.strip_prefix('/').unwrap_or(path);

    // Check exact matches
    if PUBLIC_PROXY_PATHS.contains(&normalized) {
        return true;
    }

    // Check prefix matches
    for prefix in PUBLIC_PROXY_PREFIXES {
        if normalized.starts_with(prefix) {
            return true;
        }
    }

    false
}

/// Look up user by client_id from D1 database
async fn lookup_user_by_client(env: &Env, client_id: &str) -> Result<Option<User>> {
    let db = env.d1("DB")?;

    // Query the clients table to find the user_id for this client
    let stmt = db.prepare("SELECT user_id FROM clients WHERE client_id = ?1");
    let result = stmt.bind(&[client_id.into()])?.first::<String>(Some("user_id")).await?;

    if let Some(user_id) = result {
        // Look up the full user record
        let user_stmt = db.prepare("SELECT id, github_id, github_login, email, created_at, last_login FROM users WHERE id = ?1");
        let user_row = user_stmt.bind(&[user_id.into()])?.first::<serde_json::Value>(None).await?;

        if let Some(row) = user_row {
            return Ok(Some(User {
                id: row["id"].as_str().unwrap_or_default().to_string(),
                github_id: row["github_id"].as_i64().unwrap_or(0),
                github_login: row["github_login"].as_str().unwrap_or_default().to_string(),
                email: row["email"].as_str().map(|s| s.to_string()),
                created_at: row["created_at"].as_str().unwrap_or_default().to_string(),
                last_login: row["last_login"].as_str().map(|s| s.to_string()),
            }));
        }
    }

    Ok(None)
}

/// Proxy HTTP requests to claudecodeui instances
pub async fn proxy_to_client(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Get the proxy path early to check if it's a public resource
    let proxy_path = ctx.param("path").unwrap_or(&"".to_string()).clone();

    // Get client ID from path parameter
    let client_id = ctx
        .param("id")
        .ok_or("Missing client ID")?
        .clone();

    let is_public = is_public_path(&proxy_path);

    // Try to authenticate the user
    let user: Option<User> = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(user) => Some(user),
        Err(redirect) => {
            if is_public {
                // For public paths, try to look up user by client_id in D1
                match lookup_user_by_client(&ctx.env, &client_id).await {
                    Ok(Some(user)) => Some(user),
                    Ok(None) => {
                        // Client not found in D1, can't route
                        return Response::error("Client not found", 404);
                    }
                    Err(e) => {
                        console_log!("D1 lookup error: {:?}", e);
                        return Response::error("Internal error", 500);
                    }
                }
            } else {
                // For non-public paths, check if this is a fetch request
                // Sec-Fetch-Mode: navigate = page load, cors/no-cors/same-origin = fetch
                let is_fetch = req
                    .headers()
                    .get("Sec-Fetch-Mode")
                    .ok()
                    .flatten()
                    .map(|m| m != "navigate")
                    .unwrap_or(false);

                if is_fetch {
                    // Return 401 for fetch requests to avoid CORS redirect issues
                    return Response::error("Unauthorized", 401);
                }
                return Ok(redirect);
            }
        }
    };

    // At this point, we should have a user
    let user = user.ok_or("No user found")?;

    // Get query string from original request
    let url = req.url()?;
    let query_string = url.query().map(|q| q.to_string());

    // Collect headers (filter out hop-by-hop headers)
    let mut headers: Vec<(String, String)> = Vec::new();
    let hop_by_hop = [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailers",
        "transfer-encoding",
        "upgrade",
        "host",
    ];

    for (key, value) in req.headers() {
        let key_lower = key.to_lowercase();
        if !hop_by_hop.contains(&key_lower.as_str()) {
            headers.push((key, value));
        }
    }

    // Add orchestrator user info headers for auto-authentication
    // claudecodeui can use these to auto-login the user without requiring separate auth
    headers.push((
        "X-Orchestrator-User-Id".to_string(),
        user.github_id.to_string(),
    ));
    headers.push((
        "X-Orchestrator-Username".to_string(),
        user.github_login.clone(),
    ));

    // Get request body if present
    let body = if req.method() != Method::Get && req.method() != Method::Head {
        req.text().await.ok()
    } else {
        None
    };

    // Build proxy request
    let proxy_req = ProxyRequest {
        method: req.method().to_string(),
        path: format!("/{}", proxy_path),
        headers,
        body,
        query: query_string,
    };

    // Forward to user's Durable Object
    let namespace = ctx.env.durable_object("USER_HUB")?;
    let id = namespace.id_from_name(&user.id)?;
    let stub = id.get_stub()?;

    // Create request to DO's proxy endpoint
    let do_url = format!("https://do/proxy/{}", client_id);
    let mut init = RequestInit::new();
    init.with_method(Method::Post);

    let body_json = serde_json::to_string(&proxy_req)?;
    let do_headers = Headers::new();
    do_headers.set("Content-Type", "application/json")?;
    init.with_headers(do_headers);
    init.with_body(Some(JsValue::from_str(&body_json)));

    let do_req = Request::new_with_init(&do_url, &init)?;
    let do_resp = stub.fetch_with_request(do_req).await?;

    // Check if we got a successful proxy response
    if do_resp.status_code() != 200 {
        return Ok(do_resp);
    }

    // Parse the proxy response
    let mut do_resp_mut = do_resp;
    let resp_text = do_resp_mut.text().await?;
    let proxy_resp: ProxyResponse = serde_json::from_str(&resp_text)
        .map_err(|e| Error::RustError(format!("Failed to parse proxy response: {}", e)))?;

    // Build the response to return to the client
    let mut resp_headers = Headers::new();
    for (key, value) in &proxy_resp.headers {
        let key_lower = key.to_lowercase();
        // Skip hop-by-hop headers in response too
        if !hop_by_hop.contains(&key_lower.as_str()) {
            let _ = resp_headers.set(key, value);
        }
    }

    // URL rewriting is handled by claudecodeui (it receives proxy_base in the request)
    let response_body = proxy_resp.body;

    // Create response with the proxied status and body
    // We need to create a new response with the correct status
    // worker-rs doesn't have a clean way to set status, so we rebuild it
    let response = if proxy_resp.status >= 400 {
        Response::error(&response_body, proxy_resp.status)
            .map(|r| r.with_headers(resp_headers))?
    } else {
        Response::ok(response_body)?.with_headers(resp_headers)
    };

    Ok(response)
}
