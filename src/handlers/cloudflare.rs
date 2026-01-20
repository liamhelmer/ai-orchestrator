use serde::{Deserialize, Serialize};
use worker::*;

use crate::auth::AuthMiddleware;

/// Cloudflare API response structure
#[derive(Debug, Deserialize)]
struct CloudflareResponse {
    success: bool,
    errors: Vec<CloudflareError>,
    #[serde(default)]
    messages: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CloudflareError {
    code: i32,
    message: String,
}

/// Request body for cache purge by prefix
#[derive(Debug, Serialize)]
struct PurgeCacheByPrefixRequest {
    prefixes: Vec<String>,
}

/// Purge Cloudflare cache for a specific client's proxy URLs
pub async fn purge_client_cache(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Check authentication
    let _user = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(user) => user,
        Err(redirect) => return Ok(redirect),
    };

    // Get client ID from path parameter
    let client_id = ctx.param("id").ok_or("Missing client ID")?.clone();

    // Get Cloudflare credentials from environment
    let zone_id = match ctx.env.secret("CLOUDFLARE_ZONE_ID") {
        Ok(secret) => secret.to_string(),
        Err(_) => {
            return Response::error(
                "Cloudflare Zone ID not configured. Set CLOUDFLARE_ZONE_ID secret.",
                500,
            );
        }
    };

    let api_token = match ctx.env.secret("CLOUDFLARE_API_TOKEN") {
        Ok(secret) => secret.to_string(),
        Err(_) => {
            return Response::error(
                "Cloudflare API token not configured. Set CLOUDFLARE_API_TOKEN secret.",
                500,
            );
        }
    };

    // Build the cache purge request
    // Purge all URLs under /clients/{client_id}/proxy/
    let prefix = format!("/clients/{}/proxy/", client_id);

    // We need the full URL with hostname for Cloudflare's purge API
    // Get the hostname from the request
    let url = req.url()?;
    let host = url.host_str().unwrap_or("ai-orchestrator.liam-helmer-428.workers.dev");
    let scheme = url.scheme();

    let full_prefix = format!("{}://{}{}", scheme, host, prefix);

    let purge_request = PurgeCacheByPrefixRequest {
        prefixes: vec![full_prefix.clone()],
    };

    // Call Cloudflare API to purge cache
    let cf_url = format!(
        "https://api.cloudflare.com/client/v4/zones/{}/purge_cache",
        zone_id
    );

    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {}", api_token))?;
    headers.set("Content-Type", "application/json")?;

    let body = serde_json::to_string(&purge_request)?;

    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    init.with_headers(headers);
    init.with_body(Some(wasm_bindgen::JsValue::from_str(&body)));

    let cf_req = Request::new_with_init(&cf_url, &init)?;

    let mut cf_response = Fetch::Request(cf_req).send().await?;
    let response_text = cf_response.text().await?;

    // Parse Cloudflare response
    let cf_result: CloudflareResponse = serde_json::from_str(&response_text)
        .map_err(|e| Error::RustError(format!("Failed to parse Cloudflare response: {}", e)))?;

    if cf_result.success {
        console_log!("[CLOUDFLARE] Successfully purged cache for prefix: {}", full_prefix);

        // Check if this is an HTMX request
        let is_htmx = req.headers().get("HX-Request")?.is_some();

        if is_htmx {
            // Return a success notification for HTMX
            Response::from_html(format!(
                r#"<div class="toast-success" id="purge-toast">
                    Cache purged for {}
                    <script>
                        setTimeout(() => document.getElementById('purge-toast')?.remove(), 3000);
                    </script>
                </div>"#,
                client_id
            ))
        } else {
            Response::ok("Cache purged successfully")
        }
    } else {
        let error_msg = cf_result
            .errors
            .iter()
            .map(|e| format!("[{}] {}", e.code, e.message))
            .collect::<Vec<_>>()
            .join(", ");

        console_error!("[CLOUDFLARE] Cache purge failed: {}", error_msg);
        Response::error(format!("Cache purge failed: {}", error_msg), 500)
    }
}
