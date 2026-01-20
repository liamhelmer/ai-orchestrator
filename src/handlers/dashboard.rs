use worker::*;

use crate::auth::AuthMiddleware;
use crate::templates;

/// Dashboard page - requires authentication
pub async fn dashboard(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    // Check authentication
    let user = match AuthMiddleware::require_auth(&req, &ctx.env).await? {
        Ok(user) => user,
        Err(redirect) => return Ok(redirect),
    };

    // Render dashboard with user info
    Response::from_html(templates::render_dashboard(&user))
}
