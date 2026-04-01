use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use tracing::debug;

use crate::config::AuthConfig;
use crate::credentials;

/// Apply HTTP Basic Auth middleware to a router.
pub fn apply(router: Router, config: &AuthConfig) -> Router {
    router.layer(middleware::from_fn_with_state(
        config.clone(),
        auth_middleware,
    ))
}

async fn auth_middleware(
    State(config): State<AuthConfig>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    let Some(header_value) = auth_header else {
        debug!("missing Authorization header");
        return unauthorized_response();
    };

    let Some(encoded) = header_value.strip_prefix("Basic ") else {
        debug!("Authorization header is not Basic scheme");
        return unauthorized_response();
    };

    let decoded = match BASE64.decode(encoded) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                debug!("Authorization header contains invalid UTF-8");
                return unauthorized_response();
            }
        },
        Err(_) => {
            debug!("Authorization header contains invalid base64");
            return unauthorized_response();
        }
    };

    let Some((username, password)) = decoded.split_once(':') else {
        debug!("Authorization header missing ':' separator");
        return unauthorized_response();
    };

    if username != config.username || !credentials::verify_password(password, &config.password_hash)
    {
        debug!("invalid credentials for user '{}'", username);
        return unauthorized_response();
    }

    next.run(request).await
}

fn unauthorized_response() -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("www-authenticate", "Basic realm=\"Rusty Photon\"")
        .body(Body::empty())
        .unwrap()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt as _;

    fn test_config() -> AuthConfig {
        let hash = credentials::hash_password("test-password").unwrap();
        AuthConfig {
            username: "testuser".to_string(),
            password_hash: hash,
        }
    }

    fn test_router() -> Router {
        let config = test_config();
        let app = Router::new().route("/test", get(|| async { "ok" }));
        apply(app, &config)
    }

    fn basic_auth_header(username: &str, password: &str) -> String {
        let encoded = BASE64.encode(format!("{username}:{password}"));
        format!("Basic {encoded}")
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // argon2 hashing in test_config() is too slow under Miri
    async fn valid_credentials_returns_200() {
        let app = test_router();
        let request = Request::builder()
            .uri("/test")
            .header(
                "authorization",
                basic_auth_header("testuser", "test-password"),
            )
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // argon2 hashing in test_config() is too slow under Miri
    async fn wrong_password_returns_401() {
        let app = test_router();
        let request = Request::builder()
            .uri("/test")
            .header("authorization", basic_auth_header("testuser", "wrong"))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // argon2 hashing in test_config() is too slow under Miri
    async fn wrong_username_returns_401() {
        let app = test_router();
        let request = Request::builder()
            .uri("/test")
            .header(
                "authorization",
                basic_auth_header("baduser", "test-password"),
            )
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // argon2 hashing in test_config() is too slow under Miri
    async fn missing_auth_header_returns_401() {
        let app = test_router();
        let request = Request::builder().uri("/test").body(Body::empty()).unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // argon2 hashing in test_config() is too slow under Miri
    async fn malformed_auth_header_returns_401() {
        let app = test_router();
        let request = Request::builder()
            .uri("/test")
            .header("authorization", "Bearer some-token")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // argon2 hashing in test_config() is too slow under Miri
    async fn response_401_includes_www_authenticate_header() {
        let app = test_router();
        let request = Request::builder().uri("/test").body(Body::empty()).unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let www_auth = response
            .headers()
            .get("www-authenticate")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(www_auth, "Basic realm=\"Rusty Photon\"");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // argon2 hashing in test_config() is too slow under Miri
    async fn invalid_base64_returns_401() {
        let app = test_router();
        let request = Request::builder()
            .uri("/test")
            .header("authorization", "Basic !!!not-base64!!!")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // argon2 hashing in test_config() is too slow under Miri
    async fn missing_colon_separator_returns_401() {
        let encoded = BASE64.encode("no-colon-here");
        let app = test_router();
        let request = Request::builder()
            .uri("/test")
            .header("authorization", format!("Basic {encoded}"))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
