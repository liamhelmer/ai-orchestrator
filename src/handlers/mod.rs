mod clients;
mod cloudflare;
mod dashboard;
mod proxy;
mod tokens;
mod websocket;

pub use clients::{disconnect_client, get_client, get_client_details, get_clients};
pub use cloudflare::purge_client_cache;
pub use dashboard::dashboard;
pub use proxy::proxy_to_client;
pub use tokens::{
    close_token_modal, create_token_api, delete_token, list_tokens, list_tokens_htmx,
    revoke_token_htmx, show_token_modal, validate_token,
};
pub use websocket::websocket_upgrade;

use worker::*;

use crate::auth::AuthMiddleware;
use crate::templates;

/// Home page - login screen (redirects to dashboard if already logged in)
pub async fn home(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Check if user is already authenticated
    match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(_user) => {
            // User is logged in, redirect to dashboard
            let headers = Headers::new();
            headers.set("Location", "/dashboard")?;
            Ok(Response::empty()?.with_status(302).with_headers(headers))
        }
        Err(_) => {
            // User is not logged in, show login page
            Response::from_html(templates::render_home())
        }
    }
}

/// Health check endpoint
pub fn health(_req: Request, _ctx: RouteContext<()>) -> Result<Response> {
    Response::ok("OK")
}

/// Serve static assets from R2
pub async fn serve_static(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let path = ctx.param("path").unwrap_or(&"".to_string()).clone();

    let bucket = ctx.env.bucket("ASSETS")?;
    let object = bucket.get(&path).execute().await?;

    match object {
        Some(obj) => {
            let body = obj.body().ok_or("No body")?;
            let bytes = body.bytes().await?;

            let content_type = guess_content_type(&path);
            let headers = Headers::new();
            headers.set("Content-Type", content_type)?;
            headers.set("Cache-Control", "public, max-age=31536000")?;

            Ok(Response::from_bytes(bytes)?.with_headers(headers))
        }
        None => Response::error("Not found", 404),
    }
}

fn guess_content_type(path: &str) -> &'static str {
    if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".html") {
        "text/html"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else {
        "application/octet-stream"
    }
}
