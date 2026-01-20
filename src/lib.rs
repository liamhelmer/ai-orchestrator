use worker::*;

mod auth;
mod durable_objects;
mod handlers;
mod models;
mod templates;

pub use durable_objects::UserHub;

#[event(fetch)]
async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    Router::new()
        // Public routes
        .get_async("/", handlers::home)
        .get("/health", handlers::health)
        // Auth routes
        .get_async("/auth/github", auth::start_oauth)
        .get_async("/auth/github/callback", auth::handle_callback)
        .get_async("/auth/logout", auth::logout)
        // Protected routes (dashboard)
        .get_async("/dashboard", handlers::dashboard)
        .get_async("/clients", handlers::get_clients)
        .get_async("/clients/:id", handlers::get_client)
        .get_async("/clients/:id/details", handlers::get_client_details)
        .post_async("/clients/:id/disconnect", handlers::disconnect_client)
        .post_async("/clients/:id/purge-cache", handlers::purge_client_cache)
        // Token management API (JSON)
        .get_async("/api/tokens", handlers::list_tokens)
        .post_async("/api/tokens", handlers::create_token_api)
        .post_async("/api/tokens/:id/revoke", handlers::revoke_token_htmx)
        .delete_async("/api/tokens/:id", handlers::delete_token)
        // Token management UI (HTMX)
        .get_async("/tokens", handlers::list_tokens_htmx)
        .get_async("/tokens/new", handlers::show_token_modal)
        .get_async("/tokens/close-modal", handlers::close_token_modal)
        // WebSocket upgrade for claudecodeui connections
        .get_async("/ws/connect", handlers::websocket_upgrade)
        // HTTP proxy to claudecodeui instances
        // Root path proxy (no trailing slash)
        .get_async("/clients/:id/proxy", handlers::proxy_to_client)
        .post_async("/clients/:id/proxy", handlers::proxy_to_client)
        .put_async("/clients/:id/proxy", handlers::proxy_to_client)
        .delete_async("/clients/:id/proxy", handlers::proxy_to_client)
        .patch_async("/clients/:id/proxy", handlers::proxy_to_client)
        // Root path proxy (with trailing slash)
        .get_async("/clients/:id/proxy/", handlers::proxy_to_client)
        .post_async("/clients/:id/proxy/", handlers::proxy_to_client)
        .put_async("/clients/:id/proxy/", handlers::proxy_to_client)
        .delete_async("/clients/:id/proxy/", handlers::proxy_to_client)
        .patch_async("/clients/:id/proxy/", handlers::proxy_to_client)
        // Subpath proxy (with path after /proxy/)
        .get_async("/clients/:id/proxy/*path", handlers::proxy_to_client)
        .post_async("/clients/:id/proxy/*path", handlers::proxy_to_client)
        .put_async("/clients/:id/proxy/*path", handlers::proxy_to_client)
        .delete_async("/clients/:id/proxy/*path", handlers::proxy_to_client)
        .patch_async("/clients/:id/proxy/*path", handlers::proxy_to_client)
        // Static assets
        .get_async("/static/*path", handlers::serve_static)
        .run(req, env)
        .await
}
