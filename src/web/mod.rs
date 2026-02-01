mod ws;

use crate::server::AppState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tower_cookies::{Cookie, CookieManagerLayer, Cookies};

#[derive(RustEmbed)]
#[folder = "assets/"]
struct Asset;

#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

#[derive(Serialize)]
struct ApiResponse {
    success: bool,
    message: String,
}

#[derive(Serialize)]
struct RoomInfo {
    name: String,
    has_banner: bool,
    is_locked: bool,
}

#[derive(Serialize)]
struct RoomsResponse {
    rooms: Vec<RoomInfo>,
}

#[derive(Serialize)]
struct BrandingInfo {
    has_favicon: bool,
    has_logo: bool,
    has_header_banner: bool,
    has_background: bool,
    has_custom_css: bool,
}

#[derive(Serialize)]
struct LegalInfo {
    has_imprint: bool,
    has_privacy_policy: bool,
}

#[derive(Serialize)]
struct PublicConfigResponse {
    title: String,
    stun_servers: Vec<String>,
    branding: BrandingInfo,
    legal: LegalInfo,
}

pub fn router(state: AppState) -> Router {
    let public = Router::new()
        .route("/api/login", post(login_handler))
        .route("/api/check-auth", get(check_auth_handler))
        .route("/api/config", get(get_config_handler))
        .route("/", get(index_handler))
        .route("/assets/*file", get(static_handler))
        .route("/branding/favicon", get(favicon_handler))
        .route("/branding/logo", get(logo_handler))
        .route("/branding/header-banner", get(header_banner_handler))
        .route("/branding/room-banner/:room", get(room_banner_handler))
        .route("/branding/background", get(background_handler))
        .route("/branding/custom.css", get(custom_css_handler))
        .route("/legal/impressum", get(impressum_handler))
        .route("/legal/datenschutz", get(datenschutz_handler));

    let protected = Router::new()
        .route("/ws", get(ws::ws_handler))
        .route("/api/rooms", get(get_rooms_handler))
        .route("/api/logout", post(logout_handler))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ));

    public
        .merge(protected)
        .layer(CookieManagerLayer::new())
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

async fn require_auth(
    State(state): State<AppState>,
    cookies: Cookies,
    req: Request<Body>,
    next: Next,
) -> Response {
    let key = state.cookie_key.read().await.clone();
    let private_cookies = cookies.private(&key);

    if private_cookies.get("bt_session").is_some() {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self' wss:; media-src 'self' blob:; font-src 'self'"
            .parse()
            .unwrap(),
    );
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert("Referrer-Policy", "no-referrer".parse().unwrap());
    headers.insert(
        "Permissions-Policy",
        "microphone=(self)".parse().unwrap(),
    );
    response
}

// --- API Handlers ---

async fn get_config_handler(State(state): State<AppState>) -> Json<PublicConfigResponse> {
    let config = state.config.read().await;
    Json(PublicConfigResponse {
        title: config
            .server
            .title
            .clone()
            .unwrap_or_else(|| "Buechsentelefon".to_string()),
        stun_servers: config.webrtc.stun_servers.clone(),
        branding: BrandingInfo {
            has_favicon: config.branding.favicon_path.is_some(),
            has_logo: config.branding.logo_path.is_some(),
            has_header_banner: config.branding.header_banner_path.is_some(),
            has_background: config.branding.background_image_path.is_some(),
            has_custom_css: config.branding.custom_css_path.is_some(),
        },
        legal: LegalInfo {
            has_imprint: config.legal.imprint_path.is_some(),
            has_privacy_policy: config.legal.privacy_policy_path.is_some(),
        },
    })
}

async fn get_rooms_handler(State(state): State<AppState>) -> Json<RoomsResponse> {
    let config = state.config.read().await;
    Json(RoomsResponse {
        rooms: config
            .rooms
            .iter()
            .map(|r| RoomInfo {
                name: r.name.clone(),
                has_banner: r.banner_path.is_some(),
                is_locked: r.password_hash.is_some(),
            })
            .collect(),
    })
}

async fn login_handler(
    State(state): State<AppState>,
    cookies: Cookies,
    Json(payload): Json<LoginRequest>,
) -> Response {
    {
        let mut limiter = state.rate_limiter.write().await;
        if !limiter.check_and_record() {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ApiResponse {
                    success: false,
                    message: "Too many login attempts. Please try again later.".into(),
                }),
            )
                .into_response();
        }
    }

    let config = state.config.read().await;

    if config.verify_password(&payload.password) {
        let key = state.cookie_key.read().await.clone();
        let private_cookies = cookies.private(&key);

        let mut cookie = Cookie::new("bt_session", "authenticated");
        cookie.set_path("/");
        cookie.set_http_only(true);
        cookie.set_secure(true);
        cookie.set_same_site(tower_cookies::cookie::SameSite::Strict);

        private_cookies.add(cookie);

        return Json(ApiResponse {
            success: true,
            message: "Login successful".into(),
        })
        .into_response();
    }

    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse {
            success: false,
            message: "Invalid password".into(),
        }),
    )
        .into_response()
}

async fn check_auth_handler(State(state): State<AppState>, cookies: Cookies) -> Response {
    let key = state.cookie_key.read().await.clone();
    let private_cookies = cookies.private(&key);

    if private_cookies.get("bt_session").is_some() {
        return Json(ApiResponse {
            success: true,
            message: "Authorized".into(),
        })
        .into_response();
    }

    (
        StatusCode::UNAUTHORIZED,
        Json(ApiResponse {
            success: false,
            message: "Unauthorized".into(),
        }),
    )
        .into_response()
}

async fn logout_handler(State(state): State<AppState>, cookies: Cookies) -> impl IntoResponse {
    let key = state.cookie_key.read().await.clone();
    let private_cookies = cookies.private(&key);

    let mut cookie = Cookie::new("bt_session", "");
    cookie.set_path("/");
    private_cookies.remove(cookie);

    Json(ApiResponse {
        success: true,
        message: "Logged out".into(),
    })
}

// --- Branding Handlers ---

async fn favicon_handler(State(state): State<AppState>) -> Response {
    let config = state.config.read().await;
    serve_config_file(config.branding.favicon_path.as_deref()).await
}

async fn logo_handler(State(state): State<AppState>) -> Response {
    let config = state.config.read().await;
    serve_config_file(config.branding.logo_path.as_deref()).await
}

async fn header_banner_handler(State(state): State<AppState>) -> Response {
    let config = state.config.read().await;
    serve_config_file(config.branding.header_banner_path.as_deref()).await
}

async fn room_banner_handler(
    State(state): State<AppState>,
    Path(room_name): Path<String>,
) -> Response {
    let config = state.config.read().await;
    let path = config
        .rooms
        .iter()
        .find(|r| r.name == room_name)
        .and_then(|r| r.banner_path.as_deref());
    serve_config_file(path).await
}

async fn background_handler(State(state): State<AppState>) -> Response {
    let config = state.config.read().await;
    serve_config_file(config.branding.background_image_path.as_deref()).await
}

async fn custom_css_handler(State(state): State<AppState>) -> Response {
    let config = state.config.read().await;
    serve_config_file(config.branding.custom_css_path.as_deref()).await
}

async fn serve_config_file(path_opt: Option<&str>) -> Response {
    if let Some(path_str) = path_opt {
        let path = std::path::Path::new(path_str);
        if path.exists() {
            if let Ok(bytes) = tokio::fs::read(path).await {
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                return ([(header::CONTENT_TYPE, mime.as_ref())], Body::from(bytes))
                    .into_response();
            }
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

// --- Legal Handlers ---

async fn impressum_handler(State(state): State<AppState>) -> Response {
    let config = state.config.read().await;
    serve_config_file(config.legal.imprint_path.as_deref()).await
}

async fn datenschutz_handler(State(state): State<AppState>) -> Response {
    let config = state.config.read().await;
    serve_config_file(config.legal.privacy_policy_path.as_deref()).await
}

// --- Static Asset Handlers ---

async fn index_handler() -> impl IntoResponse {
    serve_embedded("index.html")
}

async fn static_handler(Path(file): Path<String>) -> impl IntoResponse {
    let file = file.trim_start_matches('/');
    serve_embedded(file)
}

fn serve_embedded(path: &str) -> Response {
    match Asset::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref())],
                Body::from(content.data),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
